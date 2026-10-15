//! IdentityResolver (DD.5): the last Resolve pass. It writes one SAME_AS star
//! per identity class: two or more nodes that are one thing - one package
//! across repos, a package and the project that publishes it, one proto /
//! Avro / JSON Schema message in several repos, and one contract operation
//! declared several times. SCIP symbols (DD.7) and vendored copies (DD.6)
//! are later rules over the same table.
//!
//! **Nodes only.** The pass reads nodes, their qnames and cells, never
//! another resolver's edges, so it can run anywhere in the Resolve stage and
//! a layout merge recomputes it over the union (`resolver:` emitters are
//! dropped from members and re-run), which is what keeps merge == joint
//! build. The one index it builds is the HTTP resolver's own ROUTE matcher,
//! for the route anchor below.
//!
//! **Rules.** Which kinds a rule pairs and how surely (its tier) are the code
//! domain's `CODE_TABLES.identity_rules` rows (DD.3), read by name, never a
//! local list; the keys are `glia_code_domain::same_as`'s (DD.2). Five rules:
//!
//! - `package_name` (FACT): PACKAGE_DEPs whose `same_as::dep_identity_key`
//!   agrees (`package:pypi:My_Lib` == `package:pypi:my-lib`; npm `requests`
//!   is never pypi `requests`).
//! - `manifest_name` (FACT): a PROJECT whose ORIGIN `label_source` is
//!   `manifest` (DD.4: the manifest declared the name; a dir-derived,
//!   `root` or overlay-declared label never matches) and the PACKAGE_DEPs
//!   naming it, keyed by `same_as::project_dep_key`. A class needs the
//!   PROJECT and at least one dependency. A .NET project's label is the
//!   csproj file stem (DD.4 records `manifest` for it) but no ecosystem the
//!   key maps .NET to extracts a PACKAGE_DEP, so the question of whether a
//!   stem is a declared name never reaches a class.
//! - `message_fqname` (FACT): MESSAGE_TYPEs with one qname (flavour and fully
//!   qualified name are already in it, as `MessageSchemaResolver` keys them).
//! - `contract_op` (DERIVED): contract-op DOC_SECTIONs (an ORIGIN
//!   `same_as::contract_op_key` reads) of one repo and one owning PROJECT
//!   with one match key: `METHOD` + the parameter-folded path
//!   (`normalise_http_path`, so `{id}` == `:id`), or a channel op's display.
//!   The owning PROJECT is the deepest PROJECT of the op's repo whose path is
//!   the repo root or a `/`-bounded prefix of the op's POSITION file; an op
//!   with no owning PROJECT joins no `contract_op` class (two services of a
//!   manifest-less monorepo that both declare `GET /orders` are not one op).
//! - `contract_route` (DERIVED): HTTP contract ops whose declared
//!   `(method, path)` - else, when that misses, their ORIGIN `raw_path` -
//!   reaches exactly one ROUTE through `HttpRouteMatcher` (the tiers and
//!   order `link_contract_routes` pairs DOCUMENTS with). `any`-tier hits never
//!   anchor, and an op reaching two routes anchors to neither.
//!
//! **Stars.** A class is a key with at least two distinct nodes. Its center
//! is the member with the least (qname, kind id, index of the first graph
//! holding it): a function of content and argument order, never of a NodeId,
//! so the star at qname level does not move when a RepoId does (DE.1's
//! invariance; a query-time union-find keeps its own min-NodeId handle).
//! Every other member gets one `member -> center` SAME_AS edge carrying ONE
//! EVIDENCE cell, emitter `resolver:identity`, rule `<rule>` or
//! `<rule>/diverged` when the member's and the center's contents
//! (`same_as::content_key`: a SCHEMA_FIELDS payload for a MESSAGE_TYPE or
//! DOC_SECTION) are both known and differ. Confidence is the rule's tier:
//! FACT Strong, DERIVED Medium. Every class, member set and key is a BTree,
//! so emission order is total and hash-free (the Finalize sort fixes the
//! stored order anyway). No node or node cell is written.
//!
//! fired_on, once per resolve that formed a class, after LC.3c's
//! `[evidence-rules] resolver=identity package_name=<n> ...` (edges per rule):
//! `[identity] classes=<c> edges=<e> diverged=<d> package_name=<n>
//! manifest_name=<n> message_fqname=<n> contract_op=<n> contract_route=<n>`
//! (classes per rule; `d` counts the classes with a diverged edge).
//! grep: `... 2>&1 | grep '^\[identity\] classes='`

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_activation::profile::{IdentityRule, IdentityTier};
use glia_code_domain::evidence;
use glia_code_domain::profile::CODE_TABLES;
use glia_code_domain::project_roots::LABEL_MANIFEST;
use glia_code_domain::same_as::{self, ContractOpKey, OpShape};
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Confidence, Edge, NodeId, NodeKindId, RepoId};

use super::host::str_field;
use super::http::{HttpRouteMatcher, normalise_http_path};
use super::{CrossGraphResolver, RuleTally, rule_evidence};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

/// The pass name: the EVIDENCE emitter is `resolver:identity`, the stamp the
/// engine's `resolver!("identity", ..)` row would give.
const PASS: &str = "identity";

/// The rules this pass writes, in report and emission order.
const PASS_RULES: [&str; 5] = [
    same_as::RULE_PACKAGE_NAME,
    same_as::RULE_MANIFEST_NAME,
    same_as::RULE_MESSAGE_FQNAME,
    same_as::RULE_CONTRACT_OP,
    same_as::RULE_CONTRACT_ROUTE,
];

/// SAME_AS stars over every identity class of the build (module doc).
pub struct IdentityResolver;

impl CrossGraphResolver for IdentityResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let (edges, report) = identity_edges(&merged.graphs);
        report.print();
        merged.cross_edges.extend(edges);
    }
}

/// One node a rule may pair, from the first graph holding it.
struct Candidate<'g> {
    id: NodeId,
    repo: RepoId,
    /// Index of the first graph holding the node (the center tie-break).
    graph: usize,
    kind: NodeKindId,
    qname: &'g str,
    /// The nav name: a PROJECT's manifest label.
    name: Option<&'g str>,
    cells: &'g [Cell],
    /// The contract-op key, for a kind a contract rule pairs.
    op: Option<ContractOpKey>,
}

/// A class's members, as indices into the candidate list (graph order, each
/// NodeId once), so a set of them is ordered and hash-free.
type Members = BTreeSet<usize>;

/// What one resolve found, for the two markers.
struct Report {
    tally: RuleTally,
    /// Classes formed per rule, in [`PASS_RULES`] order.
    classes: [usize; PASS_RULES.len()],
    edges: usize,
    /// Classes with at least one diverged edge.
    diverged: usize,
}

impl Report {
    fn new() -> Self {
        Self {
            tally: RuleTally::new(PASS, &PASS_RULES),
            classes: [0; PASS_RULES.len()],
            edges: 0,
            diverged: 0,
        }
    }

    /// The `[identity]` line, or `None` when no class formed.
    fn line(&self) -> Option<String> {
        let total: usize = self.classes.iter().sum();
        if total == 0 {
            return None;
        }
        let per_rule: Vec<String> = PASS_RULES
            .iter()
            .zip(self.classes)
            .map(|(r, n)| format!("{r}={n}"))
            .collect();
        Some(format!(
            "[identity] classes={total} edges={} diverged={} {}",
            self.edges,
            self.diverged,
            per_rule.join(" ")
        ))
    }

    fn print(&self) {
        self.tally.report();
        if let Some(line) = self.line() {
            eprintln!("{line}");
        }
    }
}

/// The SAME_AS edges of `graphs` and the report behind the markers.
fn identity_edges(graphs: &[RepoGraph]) -> (Vec<Edge>, Report) {
    let rule = |name: &str| CODE_TABLES.identity_rule(name);
    let [package, manifest, message, contract_op, contract_route] = PASS_RULES.map(rule);
    let mut report = Report::new();
    let mut edges = Vec::new();

    let plain_kinds: Vec<NodeKindId> = [package, manifest, message]
        .into_iter()
        .flatten()
        .flat_map(rule_kinds)
        .collect();
    let op_kinds: Vec<NodeKindId> = [contract_op, contract_route]
        .into_iter()
        .flatten()
        .flat_map(rule_kinds)
        .collect();
    let cands = collect_candidates(graphs, &plain_kinds, &op_kinds);
    if cands.is_empty() {
        return (edges, report);
    }

    let mut stars = Stars {
        cands: &cands,
        cells: BTreeMap::new(),
        report: &mut report,
        out: &mut edges,
    };
    if let Some(r) = package {
        stars.emit(0, r, package_classes(r, &cands).values());
    }
    if let Some(r) = manifest {
        stars.emit(1, r, manifest_classes(r, &cands).values());
    }
    if let Some(r) = message {
        stars.emit(2, r, message_classes(r, &cands).values());
    }
    if let Some(r) = contract_op {
        let projects = ProjectRels::of(graphs);
        stars.emit(3, r, contract_op_classes(r, &cands, &projects).values());
    }
    if let Some(r) = contract_route {
        stars.emit(4, r, contract_route_classes(r, &cands, graphs).values());
    }
    (edges, report)
}

/// The member kinds of `rule`, both sides.
fn rule_kinds(rule: &IdentityRule) -> impl Iterator<Item = NodeKindId> + '_ {
    rule.from.iter().chain(rule.to).copied()
}

/// Does `rule` pair nodes of `kind`?
fn pairs(rule: &IdentityRule, kind: NodeKindId) -> bool {
    rule.from.contains(&kind) || rule.to.contains(&kind)
}

/// Every node of a kind some rule pairs, walking `graphs` in order, each
/// NodeId once (the first graph holding it wins, as `Locator` does). A node of
/// a kind only the contract rules pair is kept only when its ORIGIN reads as
/// a contract op, so a build's markdown sections cost one ORIGIN scan each.
fn collect_candidates<'g>(
    graphs: &'g [RepoGraph],
    plain_kinds: &[NodeKindId],
    op_kinds: &[NodeKindId],
) -> Vec<Candidate<'g>> {
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut out = Vec::new();
    for (graph, g) in graphs.iter().enumerate() {
        for n in &g.nodes {
            let Some(&kind) = g.nav.kind_by_id.get(&n.id) else {
                continue;
            };
            let plain = plain_kinds.contains(&kind);
            let contract = op_kinds.contains(&kind);
            if !plain && !contract {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            if !seen.insert(n.id) {
                continue;
            }
            let op = if contract {
                same_as::contract_op_key_cells(&n.cells)
            } else {
                None
            };
            if !plain && op.is_none() {
                continue;
            }
            out.push(Candidate {
                id: n.id,
                repo: n.repo,
                graph,
                kind,
                qname,
                name: g.nav.name_by_id.get(&n.id).map(String::as_str),
                cells: &n.cells,
                op,
            });
        }
    }
    out
}

// ---- the five rules ---------------------------------------------------------

/// `package_name`: PACKAGE_DEPs by `same_as::dep_identity_key`.
fn package_classes(rule: &IdentityRule, cands: &[Candidate]) -> BTreeMap<String, Members> {
    let mut classes: BTreeMap<String, Members> = BTreeMap::new();
    for (i, c) in cands.iter().enumerate() {
        if !pairs(rule, c.kind) {
            continue;
        }
        if let Some(key) = same_as::dep_identity_key(c.qname) {
            classes.entry(key).or_default().insert(i);
        }
    }
    classes
}

/// `manifest_name`: a manifest-labelled PROJECT and the PACKAGE_DEPs naming
/// the package it declares. A key with no PROJECT, or no dependency, is no
/// class.
fn manifest_classes(rule: &IdentityRule, cands: &[Candidate]) -> BTreeMap<String, Members> {
    let mut sides: BTreeMap<String, (Members, Members)> = BTreeMap::new();
    for (i, c) in cands.iter().enumerate() {
        if !pairs(rule, c.kind) {
            continue;
        }
        if c.kind == node_kind::PROJECT {
            if let Some(key) = manifest_project_key(c) {
                sides.entry(key).or_default().0.insert(i);
            }
        } else if let Some(key) = same_as::dep_identity_key(c.qname) {
            sides.entry(key).or_default().1.insert(i);
        }
    }
    sides
        .into_iter()
        .filter(|(_, (projects, deps))| !projects.is_empty() && !deps.is_empty())
        .map(|(key, (projects, deps))| (key, projects.into_iter().chain(deps).collect()))
        .collect()
}

/// The package key a PROJECT's manifest declares: its ORIGIN must be a
/// `project_root` whose `label_source` is [`LABEL_MANIFEST`], and the label
/// is the ORIGIN's `label` - the one DD.4 recorded the source of. A label
/// JSON had to escape is read from the PROJECT's nav name instead, which the
/// walker records unescaped (the graph crate stays off serde_json).
fn manifest_project_key(c: &Candidate) -> Option<String> {
    let origin = origins(c.cells).find(|p| str_field(p, "provenance") == Some("project_root"))?;
    if str_field(origin, "label_source") != Some(LABEL_MANIFEST) {
        return None;
    }
    let label = match str_field(origin, "label") {
        Some(l) if !l.contains('\\') => l,
        _ => c.name?,
    };
    same_as::project_dep_key(str_field(origin, "ecosystem")?, label)
}

/// `message_fqname`: MESSAGE_TYPEs by exact qname.
fn message_classes(rule: &IdentityRule, cands: &[Candidate]) -> BTreeMap<String, Members> {
    let mut classes: BTreeMap<String, Members> = BTreeMap::new();
    for (i, c) in cands.iter().enumerate() {
        if pairs(rule, c.kind) {
            classes.entry(c.qname.to_string()).or_default().insert(i);
        }
    }
    classes
}

/// A contract op's class key: its repo, its owning PROJECT's path, whether it
/// is an HTTP (0) or channel (1) op, and its match key.
type OpClassKey = (u64, String, u8, String);

/// `contract_op`: ops of one repo and owning PROJECT with one match key.
fn contract_op_classes(
    rule: &IdentityRule,
    cands: &[Candidate],
    projects: &ProjectRels,
) -> BTreeMap<OpClassKey, Members> {
    let mut classes: BTreeMap<OpClassKey, Members> = BTreeMap::new();
    for (i, c) in cands.iter().enumerate() {
        if !pairs(rule, c.kind) {
            continue;
        }
        let Some(op) = &c.op else { continue };
        let Some((file, _)) = evidence::locate(c.cells) else {
            continue;
        };
        let Some(owner) = projects.owner_of(c.repo, &file) else {
            continue;
        };
        let (shape, key) = match &op.shape {
            OpShape::Http { method, path, .. } => {
                (0, format!("{method} {}", normalise_http_path(path)))
            }
            OpShape::Channel { .. } => (1, op.display.clone()),
        };
        classes
            .entry((c.repo.0, owner.to_string(), shape, key))
            .or_default()
            .insert(i);
    }
    classes
}

/// `contract_route`: HTTP ops grouped by the one ROUTE each reaches. The
/// ROUTE index is built only when some HTTP op exists.
fn contract_route_classes(
    rule: &IdentityRule,
    cands: &[Candidate],
    graphs: &[RepoGraph],
) -> BTreeMap<u64, Members> {
    let mut classes: BTreeMap<u64, Members> = BTreeMap::new();
    let ops: Vec<(usize, &str, &str, Option<&str>)> = cands
        .iter()
        .enumerate()
        .filter(|(_, c)| pairs(rule, c.kind))
        .filter_map(|(i, c)| match c.op.as_ref().map(|op| &op.shape) {
            Some(OpShape::Http {
                method,
                path,
                raw_path,
            }) => Some((i, method.as_str(), path.as_str(), raw_path.as_deref())),
            _ => None,
        })
        .collect();
    if ops.is_empty() {
        return classes;
    }
    let matcher = HttpRouteMatcher::new(graphs);
    for (i, method, path, raw_path) in ops {
        // link_contract_routes' order: the declared path, then the raw path
        // when that misses and differs.
        let mut hits = matcher.lookup(method, path);
        if hits.is_empty()
            && let Some(raw) = raw_path
            && raw != path
        {
            hits = matcher.lookup(method, raw);
        }
        let routes: BTreeSet<u64> = hits
            .iter()
            .filter(|h| h.tier != "any")
            .map(|h| h.route.0)
            .collect();
        if let (1, Some(&route)) = (routes.len(), routes.first()) {
            classes.entry(route).or_default().insert(i);
        }
    }
    classes
}

/// Every PROJECT path of every repo (`""` for the repo root), for the owning
/// project of a contract op. Only looked up, never iterated.
struct ProjectRels {
    rels: HashMap<RepoId, Vec<String>>,
}

impl ProjectRels {
    fn of(graphs: &[RepoGraph]) -> Self {
        let mut rels: HashMap<RepoId, Vec<String>> = HashMap::new();
        for g in graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::PROJECT) {
                    continue;
                }
                let Some(rel) = g
                    .nav
                    .qname_by_id
                    .get(&n.id)
                    .and_then(|q| q.strip_prefix("project:"))
                else {
                    continue;
                };
                // The walker names the repo root `project:.`.
                let rel = if rel == "." { "" } else { rel };
                let list = rels.entry(n.repo).or_default();
                if !list.iter().any(|r| r == rel) {
                    list.push(rel.to_string());
                }
            }
        }
        Self { rels }
    }

    /// The deepest PROJECT path of `repo` enclosing the repo-relative `file`:
    /// the root (`""`) encloses every file, any other path only files under
    /// it at a `/` boundary (`servicesx/` is not under `services`).
    fn owner_of(&self, repo: RepoId, file: &str) -> Option<&str> {
        self.rels
            .get(&repo)?
            .iter()
            .map(String::as_str)
            .filter(|r| {
                r.is_empty()
                    || file
                        .strip_prefix(*r)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| b.cmp(a)))
    }
}

/// The text payloads of `cells`' ORIGIN cells.
fn origins(cells: &[Cell]) -> impl Iterator<Item = &str> {
    cells
        .iter()
        .filter(|c| c.kind == cell_type::ORIGIN)
        .filter_map(|c| match &c.payload {
            CellPayload::Json(s) | CellPayload::Text(s) => Some(s.as_str()),
            CellPayload::Bytes(_) => None,
        })
}

// ---- stars -------------------------------------------------------------------

/// The emitter of one resolve's stars: the candidates, the EVIDENCE cells
/// (one per (rule, diverged), serialised once), the report and the output.
struct Stars<'a, 'g> {
    cands: &'a [Candidate<'g>],
    cells: BTreeMap<(&'static str, bool), Cell>,
    report: &'a mut Report,
    out: &'a mut Vec<Edge>,
}

impl Stars<'_, '_> {
    /// One star per class of `rule` (the `slot`th of [`PASS_RULES`]) with two
    /// or more members, in the order `classes` yields them.
    fn emit<'m>(
        &mut self,
        slot: usize,
        rule: &'static IdentityRule,
        classes: impl Iterator<Item = &'m Members>,
    ) {
        let confidence = match rule.tier {
            IdentityTier::Fact => Confidence::Strong,
            IdentityTier::Derived => Confidence::Medium,
            IdentityTier::Heuristic => Confidence::Weak,
        };
        let cands = self.cands;
        let mut emitted = 0usize;
        for members in classes.filter(|m| m.len() >= 2) {
            let Some(center) = self.center(members) else {
                continue;
            };
            let c = &cands[center];
            let center_content = same_as::content_key(c.kind, c.cells);
            let mut diverged_class = false;
            for &i in members.iter().filter(|&&i| i != center) {
                let m = &cands[i];
                let diverged = match (same_as::content_key(m.kind, m.cells), center_content) {
                    (Some(a), Some(b)) => a != b,
                    _ => false,
                };
                diverged_class |= diverged;
                let cell = self
                    .cells
                    .entry((rule.name, diverged))
                    .or_insert_with(|| {
                        rule_evidence(PASS, &same_as::evidence_rule(rule.name, diverged)).to_cell()
                    })
                    .clone();
                self.out.push(
                    Edge::new(m.id, c.id, edge_category::SAME_AS, confidence).with_cell(cell),
                );
                emitted += 1;
            }
            self.report.classes[slot] += 1;
            if diverged_class {
                self.report.diverged += 1;
            }
        }
        self.report.edges += emitted;
        self.report.tally.add(PASS_RULES[slot], emitted);
    }

    /// The member with the least (qname, kind id, first graph index); the
    /// NodeId breaks a tie only for totality (two distinct nodes of one kind
    /// and qname never share a graph).
    fn center(&self, members: &Members) -> Option<usize> {
        members.iter().copied().min_by(|&a, &b| {
            let (x, y) = (&self.cands[a], &self.cands[b]);
            x.qname
                .cmp(y.qname)
                .then(x.kind.0.cmp(&y.kind.0))
                .then(x.graph.cmp(&y.graph))
                .then(x.id.0.cmp(&y.id.0))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::evidence::Evidence;
    use glia_code_domain::{CodeNav, GRAPH_TYPE};
    use glia_core::Node;

    /// One hand-built repo graph: nodes by (kind, qname, cells).
    struct G {
        repo: RepoId,
        nodes: Vec<Node>,
        nav: CodeNav,
    }

    impl G {
        fn new(tag: &str) -> Self {
            Self {
                repo: RepoId::from_canonical(&format!("test://identity/{tag}")),
                nodes: vec![],
                nav: CodeNav::default(),
            }
        }

        fn add(&mut self, kind: NodeKindId, qname: &str, name: &str, cells: Vec<Cell>) -> NodeId {
            let id = NodeId::from_parts(GRAPH_TYPE, self.repo, kind, qname);
            self.nodes.push(Node {
                id,
                repo: self.repo,
                confidence: Confidence::Strong,
                cells,
            });
            self.nav.record(id, name, qname, kind, None);
            id
        }

        fn dep(mut self, qname: &str) -> Self {
            self.add(
                node_kind::PACKAGE_DEP,
                qname,
                qname.rsplit(':').next().unwrap_or(qname),
                vec![],
            );
            self
        }

        fn project(mut self, rel: &str, eco: &str, label: &str, source: &str) -> Self {
            let qname = format!("project:{}", if rel.is_empty() { "." } else { rel });
            let origin = format!(
                r#"{{"provenance":"project_root","ecosystem":"{eco}","manifest":"package.json","label":"{label}","label_source":"{source}","path":"{rel}"}}"#
            );
            self.add(
                node_kind::PROJECT,
                &qname,
                label,
                vec![json(cell_type::ORIGIN, &origin)],
            );
            self
        }

        fn message(mut self, qname: &str, fields: &str) -> Self {
            self.add(
                node_kind::MESSAGE_TYPE,
                qname,
                qname,
                vec![json(cell_type::SCHEMA_FIELDS, fields)],
            );
            self
        }

        /// A contract-op DOC_SECTION for `METHOD path` declared in `file`.
        fn op(mut self, qname: &str, method: &str, path: &str, file: &str) -> Self {
            let origin = format!(
                r#"{{"provenance":"contract","source":"openapi","method":"{method}","path":"{path}"}}"#
            );
            let pos = format!(r#"{{"file":"{file}","start_line":3,"end_line":9}}"#);
            self.add(
                node_kind::DOC_SECTION,
                qname,
                qname,
                vec![
                    json(cell_type::ORIGIN, &origin),
                    json(cell_type::POSITION, &pos),
                ],
            );
            self
        }

        fn route(mut self, method: &str, path: &str) -> Self {
            let method_cell = Cell {
                kind: cell_type::ROUTE_METHOD,
                payload: CellPayload::Text(method.to_string()),
            };
            self.add(
                node_kind::ROUTE,
                &format!("{method} {path}"),
                path,
                vec![method_cell],
            );
            self
        }

        fn build(self) -> RepoGraph {
            RepoGraph {
                repo: self.repo,
                nodes: self.nodes,
                edges: vec![],
                symbols: Default::default(),
                nav: self.nav,
                unresolved_calls: vec![],
                unresolved_refs: vec![],
                properties: Default::default(),
            }
        }
    }

    fn json(kind: glia_core::CellTypeId, s: &str) -> Cell {
        Cell {
            kind,
            payload: CellPayload::Json(s.to_string()),
        }
    }

    fn run(graphs: Vec<RepoGraph>) -> MergedGraph {
        let mut m = MergedGraph::new(graphs);
        m.run(&IdentityResolver);
        m
    }

    /// `(from qname, to qname, rule, confidence)` of every SAME_AS edge, sorted.
    fn stars(m: &MergedGraph) -> Vec<(String, String, String, Confidence)> {
        let q = |id: NodeId| {
            m.graphs
                .iter()
                .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
                .unwrap_or_default()
        };
        let mut out: Vec<_> = m
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SAME_AS)
            .map(|e| {
                let ev = Evidence::of(e).unwrap_or_else(|| panic!("no evidence: {e:?}"));
                assert_eq!(ev.emitter, "resolver:identity");
                assert_eq!(e.cells.len(), 1, "one EVIDENCE cell: {e:?}");
                (
                    q(e.from),
                    q(e.to),
                    ev.rule.unwrap_or_default(),
                    e.confidence,
                )
            })
            .collect();
        out.sort_by(|a, b| (&a.0, &a.1, &a.2).cmp(&(&b.0, &b.1, &b.2)));
        out
    }

    fn id_of(m: &MergedGraph, graph: usize, qname: &str) -> NodeId {
        let g = &m.graphs[graph];
        g.nodes
            .iter()
            .map(|n| n.id)
            .find(|id| g.nav.qname_by_id.get(id).map(String::as_str) == Some(qname))
            .unwrap_or_else(|| panic!("no {qname} in graph {graph}"))
    }

    fn row(
        from: &str,
        to: &str,
        rule: &str,
        c: Confidence,
    ) -> (String, String, String, Confidence) {
        (from.to_string(), to.to_string(), rule.to_string(), c)
    }

    #[test]
    fn packages_star() {
        let react = "package:npm:react";
        let m = run(vec![
            G::new("a").dep(react).build(),
            G::new("b").dep(react).build(),
            G::new("c").dep(react).build(),
        ]);
        let center = id_of(&m, 0, react);
        let same: Vec<&Edge> = m
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SAME_AS)
            .collect();
        assert_eq!(same.len(), 2, "a star, not a clique: {same:?}");
        assert!(
            same.iter().all(|e| e.to == center),
            "both into the first graph's copy"
        );
        assert_eq!(
            [same[0].from, same[1].from]
                .into_iter()
                .collect::<HashSet<_>>(),
            [id_of(&m, 1, react), id_of(&m, 2, react)]
                .into_iter()
                .collect::<HashSet<_>>()
        );
        assert_eq!(
            stars(&m),
            vec![row(react, react, "package_name", Confidence::Strong); 2]
        );
    }

    #[test]
    fn center_is_content() {
        // The same classes with the RepoIds swapped between argument slots:
        // the qname-level star is the same.
        let build = |first: &str, second: &str| {
            run(vec![
                G::new(first)
                    .dep("package:npm:react")
                    .message("message:proto:shop.A", "[1]")
                    .build(),
                G::new(second)
                    .dep("package:npm:react")
                    .message("message:proto:shop.A", "[2]")
                    .build(),
            ])
        };
        let (m1, m2) = (build("x", "y"), build("y", "x"));
        assert_ne!(m1.graphs[0].repo, m2.graphs[0].repo);
        assert_eq!(stars(&m1), stars(&m2));
        assert_eq!(stars(&m1).len(), 2);
    }

    #[test]
    fn pypi_normalised() {
        let m = run(vec![
            G::new("a").dep("package:pypi:My_Lib").build(),
            G::new("b").dep("package:pypi:my-lib").build(),
        ]);
        assert_eq!(
            stars(&m),
            [row(
                "package:pypi:my-lib",
                "package:pypi:My_Lib",
                "package_name",
                Confidence::Strong
            )],
            "the center is the least qname (`M` < `m`)"
        );
    }

    #[test]
    fn ecosystems_apart() {
        let m = run(vec![
            G::new("a").dep("package:npm:requests").build(),
            G::new("b").dep("package:pypi:requests").build(),
        ]);
        assert!(stars(&m).is_empty());
    }

    #[test]
    fn manifest_label_only() {
        let lib = |source: &str| {
            run(vec![
                G::new("mono")
                    .project("packages/utils", "npm", "utils", source)
                    .build(),
                G::new("admin").dep("package:npm:utils").build(),
            ])
        };
        assert!(
            stars(&lib("dir")).is_empty(),
            "a dir-derived label is no declared name"
        );
        assert_eq!(
            stars(&lib("manifest")),
            [row(
                "project:packages/utils",
                "package:npm:utils",
                "manifest_name",
                Confidence::Strong
            )]
        );
        // A PROJECT alone, or with no dependency naming it, is no class.
        let alone = run(vec![
            G::new("mono")
                .project("packages/utils", "npm", "utils", "manifest")
                .build(),
        ]);
        assert!(stars(&alone).is_empty());
    }

    #[test]
    fn message_diverged() {
        let q = "message:proto:shop.v1.OrderCreated";
        let base = r#"[{"name":"order_id","number":1}]"#;
        let m = run(vec![
            G::new("orders").message(q, base).build(),
            G::new("billing").message(q, base).build(),
            G::new("shipping")
                .message(
                    q,
                    r#"[{"name":"order_id","number":1},{"name":"carrier","number":3}]"#,
                )
                .build(),
        ]);
        let center = id_of(&m, 0, q);
        let rule_of = |from: NodeId| {
            m.cross_edges
                .iter()
                .find(|e| e.from == from && e.to == center && e.category == edge_category::SAME_AS)
                .and_then(Evidence::of)
                .and_then(|ev| ev.rule)
        };
        assert_eq!(rule_of(id_of(&m, 1, q)).as_deref(), Some("message_fqname"));
        assert_eq!(
            rule_of(id_of(&m, 2, q)).as_deref(),
            Some("message_fqname/diverged")
        );
        assert_eq!(stars(&m).len(), 2);
        let report = identity_edges(&m.graphs).1;
        assert_eq!(
            report.line().as_deref(),
            Some(
                "[identity] classes=1 edges=2 diverged=1 package_name=0 manifest_name=0 \
                 message_fqname=1 contract_op=0 contract_route=0"
            )
        );
    }

    #[test]
    fn contract_op_needs_project() {
        // Two projects of one repo: one op each, so no class.
        let apart = run(vec![
            G::new("mono")
                .project("svc/a", "go", "a", "manifest")
                .project("svc/b", "go", "b", "manifest")
                .op(
                    "contract::svc::a::GET:/orders",
                    "GET",
                    "/orders",
                    "svc/a/openapi.yaml",
                )
                .op(
                    "contract::svc::b::GET:/orders",
                    "GET",
                    "/orders",
                    "svc/b/openapi.yaml",
                )
                .build(),
        ]);
        assert!(stars(&apart).is_empty(), "{:?}", stars(&apart));
        // No PROJECT at all: no owner, no class.
        let bare = run(vec![
            G::new("bare")
                .op(
                    "contract::a::GET:/orders",
                    "GET",
                    "/orders",
                    "a/openapi.yaml",
                )
                .op(
                    "contract::b::GET:/orders",
                    "GET",
                    "/orders",
                    "b/openapi.yaml",
                )
                .build(),
        ]);
        assert!(stars(&bare).is_empty());
        // One PROJECT, `{id}` and `:id` spellings: one op.
        let one = run(vec![
            G::new("svc")
                .project("", "go", "example.com/svc", "manifest")
                .op(
                    "contract::docs::swagger::GET:/orders/{id}",
                    "GET",
                    "/orders/{id}",
                    "docs/swagger.yaml",
                )
                .op(
                    "contract::main::GET:/orders/:id",
                    "GET",
                    "/orders/:id",
                    "main.go",
                )
                .build(),
        ]);
        assert_eq!(
            stars(&one),
            [row(
                "contract::main::GET:/orders/:id",
                "contract::docs::swagger::GET:/orders/{id}",
                "contract_op",
                Confidence::Medium
            )]
        );
    }

    #[test]
    fn contract_route_unique() {
        // `second`: a route a second graph also declares at the same
        // `(METHOD, path)`, or one only the `ANY` fallback reaches.
        let ops = |second: Option<(&str, &str)>| {
            let auth = G::new("auth")
                .op(
                    "contract::docs::swagger::POST:/auth/login",
                    "POST",
                    "/auth/login",
                    "docs/swagger.json",
                )
                .op(
                    "contract::features::auth::feature::POST:/api/auth/login",
                    "POST",
                    "/api/auth/login",
                    "features/auth/feature.yaml",
                );
            let auth = match second {
                Some(("ANY", _)) => auth.route("ANY", "/api/auth/login"),
                _ => auth.route("POST", "/api/auth/login"),
            };
            let mut graphs = vec![auth.build()];
            if let Some((method, path)) = second.filter(|(m, _)| *m != "ANY") {
                graphs.push(G::new("gateway").route(method, path).build());
            }
            run(graphs)
        };
        assert_eq!(
            stars(&ops(None)),
            [row(
                "contract::features::auth::feature::POST:/api/auth/login",
                "contract::docs::swagger::POST:/auth/login",
                "contract_route",
                Confidence::Medium
            )],
            "one exact and one route-prefix hit on the one ROUTE"
        );
        // Both ops reach both routes: an ambiguous lookup anchors nothing.
        let two = stars(&ops(Some(("POST", "/api/auth/login"))));
        assert!(two.is_empty(), "{two:?}");
        // The `any` tier never anchors.
        let any = stars(&ops(Some(("ANY", "/api/auth/login"))));
        assert!(any.is_empty(), "{any:?}");
    }

    #[test]
    fn order_free() {
        // Distinct-qname classes (contract ops of one repo, a PROJECT and its
        // dependency) give the same edges, NodeId for NodeId, in either
        // argument order; an equal-qname class's center is the first graph's
        // copy by design, so its edge flips but its qname-level star does not.
        let graphs = || {
            vec![
                G::new("svc")
                    .project("", "npm", "@shop/money", "manifest")
                    .op("contract::b::GET:/orders", "GET", "/orders", "b.yaml")
                    .op("contract::a::GET:/orders", "GET", "/orders", "a.yaml")
                    .route("GET", "/orders")
                    .build(),
                G::new("web")
                    .dep("package:npm:@shop/money")
                    .dep("package:npm:react")
                    .build(),
                G::new("admin").dep("package:npm:react").build(),
            ]
        };
        let mut fwd = run(graphs());
        let mut rev = run(graphs().into_iter().rev().collect());
        fwd.sort_cross_edges();
        rev.sort_cross_edges();
        assert_eq!(stars(&fwd), stars(&rev));
        let distinct = |m: &MergedGraph| {
            let react: Vec<NodeId> = m
                .graphs
                .iter()
                .flat_map(|g| {
                    g.nodes
                        .iter()
                        .map(|n| n.id)
                        .filter(|id| g.nav.qname_by_id[id] == "package:npm:react")
                })
                .collect();
            m.cross_edges
                .iter()
                .filter(|e| !react.contains(&e.from))
                .cloned()
                .collect::<Vec<_>>()
        };
        assert_eq!(distinct(&fwd), distinct(&rev));
        assert_eq!(
            stars(&fwd).len(),
            4,
            "contract_op, contract_route, manifest_name, package_name: {:?}",
            stars(&fwd)
        );
    }

    /// The evidence-rules tally counts edges per rule, the `[identity]` line
    /// classes per rule; both are silent with nothing to pair.
    #[test]
    fn markers() {
        let m = run(vec![G::new("solo").dep("package:npm:react").build()]);
        let report = identity_edges(&m.graphs).1;
        assert_eq!(report.line(), None);
        assert_eq!(report.tally.line(), None);
        let graphs = vec![
            G::new("a")
                .dep("package:npm:react")
                .dep("package:npm:lodash")
                .build(),
            G::new("b")
                .dep("package:npm:react")
                .dep("package:npm:lodash")
                .build(),
            G::new("c").dep("package:npm:react").build(),
        ];
        let report = identity_edges(&graphs).1;
        assert_eq!(
            report.tally.line().as_deref(),
            Some(
                "[evidence-rules] resolver=identity package_name=3 manifest_name=0 message_fqname=0 \
                 contract_op=0 contract_route=0"
            )
        );
        assert_eq!(
            report.line().as_deref(),
            Some(
                "[identity] classes=2 edges=3 diverged=0 package_name=2 manifest_name=0 \
                 message_fqname=0 contract_op=0 contract_route=0"
            )
        );
    }
}
