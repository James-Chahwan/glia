//! IdentityResolver (DD.5): the last Resolve pass. It writes one SAME_AS star
//! per identity class: two or more nodes that are one thing - one package
//! across repos, a package and the project that publishes it, one proto /
//! Avro / JSON Schema message in several repos, one contract operation
//! declared several times, and one definition two repos' SCIP indexes name
//! (DD.7) - and, as HEURISTIC candidates, a declaration vendored into another
//! repo (DD.6).
//!
//! **Nodes only.** The pass reads nodes, their qnames and cells, never
//! another resolver's edges, so it can run anywhere in the Resolve stage and
//! a layout merge recomputes it over the union (`resolver:` emitters are
//! dropped from members and re-run), which is what keeps merge == joint
//! build. The one index it builds is the HTTP resolver's own ROUTE matcher,
//! for the route anchor below. The SCIP keys are the one input from outside
//! the graph: the engine reads each repo's snapshot and hands the pass the
//! `(node, symbol)` pairs it bound ([`IdentityResolver::with_scip`]); a merge
//! reads them from its members' recorded roots, as a joint build reads them
//! from the roots it was given.
//!
//! **Rules.** Which kinds a rule pairs and how surely (its tier) are the code
//! domain's `CODE_TABLES.identity_rules` rows (DD.3), read by name, never a
//! local list; the keys are `glia_code_domain::same_as`'s (DD.2). Eight
//! rules, in `same_as::RULES` order:
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
//! - `scip_symbol` (FACT, DD.7): definitions whose SCIP symbols agree on
//!   `same_as::scip_identity_key` - scheme, manager, package and descriptor,
//!   the version cut out (`money 1.0` and `money 1.1` of one function are one
//!   identity whose bodies may differ). The engine binds each symbol of a
//!   fresh snapshot document to one node (the SCIP stage's BIND: innermost
//!   span, exact name, an ambiguous symbol dropped) and never hands over a
//!   `local` one, so one repo names a key with one node and a class is cross
//!   repo in practice. Keys are joined through `Classes::from_pairs`: a node
//!   two symbols name joins both keys' classes into one, so no member ->
//!   center edge is written twice.
//! - `body_hash` and `path_suffix` (HEURISTIC, DD.6): vendored copies, CROSS
//!   repo only (two directories of one repo are its own business). A
//!   declaration (MODULE, FUNCTION, METHOD, CLASS, ... with a POSITION file)
//!   is keyed by (kind, its local name chain below its MODULE - `""` for the
//!   MODULE itself -, file basename) and digested by `same_as::code_digest`
//!   (xxh64 of its CODE, read from the text or from an unresolved CodeSpan
//!   alike, so a merge without the sources pairs as the joint build does;
//!   `None` under 40 bytes). Two repos' declarations of one key pair when one
//!   path is the other or a `/`-suffix of it; the stripped prefix is the
//!   pair's vendoring offset (`third_party/money` for
//!   `third_party/money/src/format.ts` vs `src/format.ts`). An offset is
//!   CONFIRMED when one of its pairs is two equal digests of a declaration
//!   below file level (a FUNCTION, CLASS, ...; a MODULE match alone confirms
//!   nothing, since scaffolds write declaration-free files - an Angular CLI
//!   `main.ts`, a Dockerfile - byte for byte into unrelated repos). In a
//!   confirmed offset equal digests are `body_hash` and two different ones
//!   `path_suffix`, MODULEs included; a path holding two declarations of one
//!   key (two `g`s in one file) pairs them only byte-identically, never as a
//!   diverged cross product. An unconfirmed offset pairs nothing,
//!   so a same-path boilerplate file in two unrelated apps (`src/index.ts`,
//!   `app.routes.ts`) never fuses them. Pairs
//!   join per rule through `Classes::from_pairs` (a file copied into three
//!   repos is one class). The copies are candidates: never collapsed, never
//!   walked (the rows' `walk: false`).
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
//! DOC_SECTION, the CODE digest for a declaration) are both known and differ
//! (always for a `path_suffix` member, never for a `body_hash` one).
//! Confidence is the rule's tier: FACT Strong, DERIVED Medium, HEURISTIC
//! Weak. Every class, member set and key is a BTree,
//! so emission order is total and hash-free (the Finalize sort fixes the
//! stored order anyway). No node or node cell is written.
//!
//! fired_on, once per resolve that formed a class, after LC.3c's
//! `[evidence-rules] resolver=identity package_name=<n> ...` (edges per rule):
//! `[identity] classes=<c> edges=<e> diverged=<d> package_name=<n>
//! manifest_name=<n> message_fqname=<n> contract_op=<n> contract_route=<n>
//! scip_symbol=<n> body_hash=<n> path_suffix=<n> offsets=<confirmed>/<seen>`
//! (classes per rule; `d` counts the classes with a diverged edge; `seen` the
//! vendoring offsets any declaration pair had, `confirmed` those an identical
//! pair confirmed).
//! grep: `... 2>&1 | grep '^\[identity\] classes='`

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_activation::algo::classes::Classes;
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

/// The rules this pass writes, in report and emission order: every identity
/// rule of the code domain, in its order.
const PASS_RULES: [&str; 8] = same_as::RULES;

/// SAME_AS stars over every identity class of the build (module doc). The
/// `scip_symbol` rule pairs only the `(node, SCIP symbol)` keys it is given
/// ([`Self::with_scip`]); [`Self::new`] has none, so that rule forms nothing.
pub struct IdentityResolver<'a> {
    scip: &'a [(NodeId, String)],
}

impl IdentityResolver<'static> {
    /// The resolver with no SCIP keys.
    pub fn new() -> Self {
        Self { scip: &[] }
    }
}

impl Default for IdentityResolver<'static> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> IdentityResolver<'a> {
    /// The resolver with `scip`: each definition a repo's SCIP snapshot bound
    /// and the symbol it bound it by (the engine's `identity_symbols`). Order
    /// and repeats are free: classes are keyed sets.
    pub fn with_scip(scip: &'a [(NodeId, String)]) -> Self {
        Self { scip }
    }
}

impl CrossGraphResolver for IdentityResolver<'_> {
    fn resolve(&self, merged: &mut MergedGraph) {
        let (edges, report) = identity_edges(&merged.graphs, self.scip);
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
    /// Vendoring offsets confirmed by an identical pair, and every offset a
    /// declaration pair had.
    offsets_confirmed: usize,
    offsets_seen: usize,
}

impl Report {
    fn new() -> Self {
        Self {
            tally: RuleTally::new(PASS, &PASS_RULES),
            classes: [0; PASS_RULES.len()],
            edges: 0,
            diverged: 0,
            offsets_confirmed: 0,
            offsets_seen: 0,
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
            "[identity] classes={total} edges={} diverged={} {} offsets={}/{}",
            self.edges,
            self.diverged,
            per_rule.join(" "),
            self.offsets_confirmed,
            self.offsets_seen
        ))
    }

    fn print(&self) {
        self.tally.report();
        if let Some(line) = self.line() {
            eprintln!("{line}");
        }
    }
}

/// The SAME_AS edges of `graphs` (with the SCIP keys `scip`) and the report
/// behind the markers.
fn identity_edges(graphs: &[RepoGraph], scip: &[(NodeId, String)]) -> (Vec<Edge>, Report) {
    let rule = |name: &str| CODE_TABLES.identity_rule(name);
    let [
        package,
        manifest,
        message,
        contract_op,
        contract_route,
        scip_symbol,
        body_hash,
        path_suffix,
    ] = PASS_RULES.map(rule);
    let mut report = Report::new();
    let mut edges = Vec::new();

    // The vendored rules pair across repos only, so a one-repo build never
    // collects, locates or digests a declaration for them.
    let multi_repo = graphs
        .first()
        .is_some_and(|first| graphs.iter().any(|g| g.repo != first.repo));
    let vendored = if multi_repo {
        [body_hash, path_suffix]
    } else {
        [None, None]
    };
    let plain_kinds: Vec<NodeKindId> = [package, manifest, message]
        .into_iter()
        .chain(vendored)
        .flatten()
        .flat_map(rule_kinds)
        .collect();
    let op_kinds: Vec<NodeKindId> = [contract_op, contract_route]
        .into_iter()
        .flatten()
        .flat_map(rule_kinds)
        .collect();
    // The SCIP rule takes only the nodes a snapshot keyed: lookup only.
    let keyed = Keyed {
        kinds: scip_symbol
            .filter(|_| !scip.is_empty())
            .into_iter()
            .flat_map(rule_kinds)
            .collect(),
        ids: scip.iter().map(|(id, _)| *id).collect(),
    };
    let cands = collect_candidates(graphs, &plain_kinds, &op_kinds, &keyed);
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
    if let Some(r) = scip_symbol.filter(|_| !scip.is_empty()) {
        stars.emit(5, r, scip_classes(r, &cands, scip).iter());
    }
    let [body, suffix] = vendored;
    if body.is_some() || suffix.is_some() {
        let decls = vendored_candidates(graphs, &cands, &vendored);
        let found = vendored_pairs(&offset_groups(&decls), &cands, body, suffix);
        stars.report.offsets_confirmed = found.confirmed;
        stars.report.offsets_seen = found.seen;
        if let Some(r) = body {
            stars.emit(6, r, joined_classes(&found.body_hash, &cands).iter());
        }
        if let Some(r) = suffix {
            stars.emit(7, r, joined_classes(&found.path_suffix, &cands).iter());
        }
    }
    (edges, report)
}

/// The nodes the SCIP rule may pair: of a kind it pairs, and keyed by a
/// snapshot. Both are lookup-only.
struct Keyed {
    kinds: Vec<NodeKindId>,
    ids: HashSet<NodeId>,
}

impl Keyed {
    fn takes(&self, kind: NodeKindId, id: NodeId) -> bool {
        self.kinds.contains(&kind) && self.ids.contains(&id)
    }
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
/// a contract op, so a build's markdown sections cost one ORIGIN scan each;
/// a node of a kind only the SCIP rule pairs only when a snapshot keyed it.
fn collect_candidates<'g>(
    graphs: &'g [RepoGraph],
    plain_kinds: &[NodeKindId],
    op_kinds: &[NodeKindId],
    keyed: &Keyed,
) -> Vec<Candidate<'g>> {
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut out = Vec::new();
    for (graph, g) in graphs.iter().enumerate() {
        for n in &g.nodes {
            let Some(&kind) = g.nav.kind_by_id.get(&n.id) else {
                continue;
            };
            let plain = plain_kinds.contains(&kind) || keyed.takes(kind, n.id);
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

// ---- the keyed rules ---------------------------------------------------------

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

/// `scip_symbol`: the candidates `scip` keys, grouped by
/// `same_as::scip_identity_key` (the symbol minus its version) and joined
/// transitively (module doc), as candidate-index member sets in member order.
/// A key one node holds is no class; a symbol that is no global SCIP symbol
/// keys nothing.
fn scip_classes(
    rule: &IdentityRule,
    cands: &[Candidate],
    scip: &[(NodeId, String)],
) -> Vec<Members> {
    // Lookup only, never iterated: the order below is `scip`'s keys'.
    let at: HashMap<NodeId, usize> = cands
        .iter()
        .enumerate()
        .filter(|(_, c)| pairs(rule, c.kind))
        .map(|(i, c)| (c.id, i))
        .collect();
    let mut keys: BTreeMap<String, Members> = BTreeMap::new();
    for (id, symbol) in scip {
        let (Some(&i), Some(key)) = (at.get(id), same_as::scip_identity_key(symbol)) else {
            continue;
        };
        keys.entry(key.key).or_default().insert(i);
    }
    let links: Vec<(usize, usize)> = keys
        .values()
        .filter_map(|m| Some((*m.first()?, m)))
        .flat_map(|(first, m)| m.iter().skip(1).map(move |&i| (first, i)))
        .collect();
    joined_classes(&links, cands)
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

// ---- vendored copies (DD.6) --------------------------------------------------

/// The most nav steps [`local_chain`] walks from a declaration up to its
/// MODULE; a longer chain, or a parent cycle, drops the declaration.
const MAX_CHAIN: usize = 64;

/// One declaration of a vendored-copy index: its candidate and its CODE
/// digest (`same_as::code_digest`: `None` without CODE or under 40 bytes).
#[derive(Clone, Copy)]
struct Decl {
    cand: usize,
    digest: Option<u64>,
}

/// What two copies of one declaration share wherever they sit: the kind id,
/// the local name chain below the MODULE and the file basename.
type DeclKey = (u32, String, String);

/// One repo's declarations: key -> repo-relative path -> declarations.
type RepoDecls = BTreeMap<DeclKey, BTreeMap<String, Vec<Decl>>>;

/// A vendoring offset between two repos (indices in first-graph order, the
/// first one less): the prefix stripped on the first repo's side and on the
/// second's, without the joining `/`. One is `""`; both are for identical
/// paths.
type OffsetKey = (usize, usize, String, String);

/// Two declarations of one key in two repos whose paths are path-suffix
/// related. `ambiguous` when either path holds more than one declaration of
/// the key: such a pair is a copy only when the digests agree.
struct DeclPair {
    a: Decl,
    b: Decl,
    ambiguous: bool,
}

/// What the vendored rules found: candidate pairs per rule, and the offsets.
#[derive(Default)]
struct VendoredPairs {
    body_hash: Vec<(usize, usize)>,
    path_suffix: Vec<(usize, usize)>,
    confirmed: usize,
    seen: usize,
}

/// Every candidate of a kind a vendored rule pairs, with a POSITION file and
/// a local name chain, indexed per repo (in first-graph order).
fn vendored_candidates(
    graphs: &[RepoGraph],
    cands: &[Candidate],
    rules: &[Option<&'static IdentityRule>],
) -> Vec<RepoDecls> {
    let mut repos: Vec<RepoId> = Vec::new();
    for g in graphs {
        if !repos.contains(&g.repo) {
            repos.push(g.repo);
        }
    }
    let mut out: Vec<RepoDecls> = repos.iter().map(|_| RepoDecls::new()).collect();
    for (i, c) in cands.iter().enumerate() {
        if !rules.iter().flatten().any(|r| pairs(r, c.kind)) {
            continue;
        }
        let Some(r) = repos.iter().position(|&r| r == c.repo) else {
            continue;
        };
        let Some((file, _)) = evidence::locate(c.cells) else {
            continue;
        };
        let Some(local) = local_chain(&graphs[c.graph], c.id) else {
            continue;
        };
        let base = file.rsplit('/').next().unwrap_or_default().to_string();
        out[r]
            .entry((c.kind.0, local, base))
            .or_default()
            .entry(file)
            .or_default()
            .push(Decl {
                cand: i,
                digest: same_as::code_digest(c.cells),
            });
    }
    out
}

/// The nav names from `id` up to, excluding, its MODULE, joined by `::`
/// (`""` for a MODULE). `None` with no MODULE ancestor, a nameless step, or
/// more than [`MAX_CHAIN`] steps (which a parent cycle always takes).
fn local_chain(g: &RepoGraph, id: NodeId) -> Option<String> {
    let mut names: Vec<&str> = Vec::new();
    let mut cur = id;
    for _ in 0..=MAX_CHAIN {
        if *g.nav.kind_by_id.get(&cur)? == node_kind::MODULE {
            names.reverse();
            return Some(names.join("::"));
        }
        names.push(g.nav.name_by_id.get(&cur)?);
        cur = *g.nav.parent_of.get(&cur)?;
    }
    None
}

/// `path` itself (when `with_self`) and every proper `/`-suffix of it, each as
/// `(stripped prefix without its '/', suffix)`. `/` is ASCII, so every cut is
/// a char boundary.
fn suffixes(path: &str, with_self: bool) -> impl Iterator<Item = (&str, &str)> {
    with_self
        .then_some(("", path))
        .into_iter()
        .chain(
            path.match_indices('/')
                .map(move |(i, _)| (&path[..i], &path[i + 1..])),
        )
        .filter(|(_, s)| !s.is_empty())
}

/// Every declaration pair across two repos, grouped by vendoring offset. For
/// each repo pair and each key both hold, every path of one side is looked
/// up in the other side's paths under itself (first side only, so an
/// identical path pairs once) and under each proper `/`-suffix: O(path depth)
/// lookups per path, never an all-pairs scan of a key's bucket.
fn offset_groups(repos: &[RepoDecls]) -> BTreeMap<OffsetKey, Vec<DeclPair>> {
    let mut groups: BTreeMap<OffsetKey, Vec<DeclPair>> = BTreeMap::new();
    for (x, rx) in repos.iter().enumerate() {
        for (y, ry) in repos.iter().enumerate().skip(x + 1) {
            for (key, px) in rx {
                let Some(py) = ry.get(key) else {
                    continue;
                };
                for (path, dx) in px {
                    for (strip, s) in suffixes(path, true) {
                        if let Some(dy) = py.get(s) {
                            let offset = (x, y, strip.to_string(), String::new());
                            push_pairs(groups.entry(offset).or_default(), dx, dy);
                        }
                    }
                }
                for (path, dy) in py {
                    for (strip, s) in suffixes(path, false) {
                        if let Some(dx) = px.get(s) {
                            let offset = (x, y, String::new(), strip.to_string());
                            push_pairs(groups.entry(offset).or_default(), dx, dy);
                        }
                    }
                }
            }
        }
    }
    groups
}

fn push_pairs(group: &mut Vec<DeclPair>, xs: &[Decl], ys: &[Decl]) {
    let ambiguous = xs.len() > 1 || ys.len() > 1;
    for &a in xs {
        for &b in ys {
            group.push(DeclPair { a, b, ambiguous });
        }
    }
}

/// The `body_hash` and `path_suffix` pairs of the confirmed offsets (module
/// doc), each kept only for kinds its rule pairs. A pair confirms its offset
/// when its digests agree and it is no MODULE pair.
fn vendored_pairs(
    groups: &BTreeMap<OffsetKey, Vec<DeclPair>>,
    cands: &[Candidate],
    body: Option<&IdentityRule>,
    suffix: Option<&IdentityRule>,
) -> VendoredPairs {
    let identical = |p: &DeclPair| matches!((p.a.digest, p.b.digest), (Some(x), Some(y)) if x == y);
    // A whole-file match confirms nothing on its own: a declaration-free file
    // (an Angular CLI `main.ts`, a Dockerfile, a config file) is what a
    // scaffold writes byte for byte into unrelated repos. A copied library
    // shows identical declarations below file level.
    let confirms = |p: &DeclPair| identical(p) && cands[p.a.cand].kind != node_kind::MODULE;
    let takes = |rule: Option<&IdentityRule>, p: &DeclPair| {
        rule.is_some_and(|r| pairs(r, cands[p.a.cand].kind) && pairs(r, cands[p.b.cand].kind))
    };
    let mut out = VendoredPairs {
        seen: groups.len(),
        ..VendoredPairs::default()
    };
    for group in groups.values() {
        if !group.iter().any(confirms) {
            continue;
        }
        out.confirmed += 1;
        for p in group {
            // A pair without two digests (no CODE, or under 40 bytes) is
            // neither a copy nor a diverged one.
            let (Some(x), Some(y)) = (p.a.digest, p.b.digest) else {
                continue;
            };
            let pair = (p.a.cand, p.b.cand);
            if x == y {
                if takes(body, p) {
                    out.body_hash.push(pair);
                }
            } else if !p.ambiguous && takes(suffix, p) {
                out.path_suffix.push(pair);
            }
        }
    }
    out
}

/// The classes `pairs` (candidate indices) join, transitively, through
/// DD.1's `Classes::from_pairs`, as candidate-index member sets in member
/// order (the vendored rules' pairs, and the SCIP rule's key links).
fn joined_classes(pairs: &[(usize, usize)], cands: &[Candidate]) -> Vec<Members> {
    if pairs.is_empty() {
        return Vec::new();
    }
    let classes = Classes::from_pairs(pairs.iter().map(|&(a, b)| (cands[a].id, cands[b].id)));
    // Lookup only, never iterated: the order below is the member sets'.
    let at: HashMap<NodeId, usize> = pairs
        .iter()
        .flat_map(|&(a, b)| [a, b])
        .map(|i| (cands[i].id, i))
        .collect();
    let mut out: Vec<Members> = classes
        .classes()
        .map(|(_, ids)| ids.iter().filter_map(|id| at.get(id).copied()).collect())
        .collect();
    out.sort();
    out
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
    use glia_code_domain::code_span::CodeSpan;
    use glia_code_domain::evidence::Evidence;
    use glia_code_domain::{CodeNav, GRAPH_TYPE};
    use glia_core::Node;

    /// One hand-built repo graph: nodes by (kind, qname, cells).
    struct G {
        repo: RepoId,
        nodes: Vec<Node>,
        nav: CodeNav,
        /// The MODULE of each file [`G::func`] declared into.
        modules: BTreeMap<String, NodeId>,
    }

    impl G {
        fn new(tag: &str) -> Self {
            Self {
                repo: RepoId::from_canonical(&format!("test://identity/{tag}")),
                nodes: vec![],
                nav: CodeNav::default(),
                modules: BTreeMap::new(),
            }
        }

        fn add(&mut self, kind: NodeKindId, qname: &str, name: &str, cells: Vec<Cell>) -> NodeId {
            self.add_under(kind, qname, name, cells, None)
        }

        fn add_under(
            &mut self,
            kind: NodeKindId,
            qname: &str,
            name: &str,
            cells: Vec<Cell>,
            parent: Option<NodeId>,
        ) -> NodeId {
            let id = NodeId::from_parts(GRAPH_TYPE, self.repo, kind, qname);
            self.nodes.push(Node {
                id,
                repo: self.repo,
                confidence: Confidence::Strong,
                cells,
            });
            self.nav.record(id, name, qname, kind, parent);
            id
        }

        /// A FUNCTION `name` with CODE `code` in `file` (repo-relative), under
        /// the file's MODULE, which carries no CODE (so only the functions
        /// pair). Qnames are the path's `::` form, as the walker mints them.
        fn func(mut self, file: &str, name: &str, code: CellPayload) -> Self {
            let module_q = file.trim_end_matches(".ts").replace('/', "::");
            let pos = |line: u32| {
                json(
                    cell_type::POSITION,
                    &format!(r#"{{"file":"{file}","start_line":{line}}}"#),
                )
            };
            let module = match self.modules.get(file) {
                Some(&m) => m,
                None => {
                    let leaf = module_q.rsplit("::").next().unwrap_or_default().to_string();
                    let m = self.add(node_kind::MODULE, &module_q, &leaf, vec![pos(0)]);
                    self.modules.insert(file.to_string(), m);
                    m
                }
            };
            let code = Cell {
                kind: cell_type::CODE,
                payload: code,
            };
            self.add_under(
                node_kind::FUNCTION,
                &format!("{module_q}::{name}"),
                name,
                vec![code, pos(2)],
                Some(module),
            );
            self
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
        m.run(&IdentityResolver::new());
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
        let report = identity_edges(&m.graphs, &[]).1;
        assert_eq!(
            report.line().as_deref(),
            Some(
                "[identity] classes=1 edges=2 diverged=1 package_name=0 manifest_name=0 \
                 message_fqname=1 contract_op=0 contract_route=0 scip_symbol=0 body_hash=0 path_suffix=0 \
                 offsets=0/0"
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

    // ---- vendored copies (DD.6) ----------------------------------------------

    /// `text` padded with spaces to `n` bytes: a CODE body of an exact size.
    fn body(n: usize, text: &str) -> CellPayload {
        assert!(text.len() <= n, "{text:?} is longer than {n}");
        CellPayload::Text(format!("{text:<n$}"))
    }

    /// `f`'s body, byte-identical in every copy (60 bytes).
    fn f60() -> CellPayload {
        body(60, "export function f(a: number) { return a * 2 + 1; }")
    }

    /// `g`'s body as the library has it, and as a copy diverged from it.
    fn g_lib() -> CellPayload {
        body(60, "export function g(s: string) { return s.trim(); }")
    }

    fn g_fork() -> CellPayload {
        body(
            60,
            "export function g(s: string) { return s.toLowerCase(); }",
        )
    }

    #[test]
    fn vendored_identical() {
        let m = run(vec![
            G::new("lib").func("lib/a.ts", "f", f60()).build(),
            G::new("app").func("vendor/lib/a.ts", "f", f60()).build(),
        ]);
        assert_eq!(
            stars(&m),
            [row(
                "vendor::lib::a::f",
                "lib::a::f",
                "body_hash",
                Confidence::Weak
            )]
        );
    }

    #[test]
    fn vendored_diverged() {
        let m = run(vec![
            G::new("lib")
                .func("lib/a.ts", "f", f60())
                .func("lib/a.ts", "g", g_lib())
                .build(),
            G::new("app")
                .func("vendor/lib/a.ts", "f", f60())
                .func("vendor/lib/a.ts", "g", g_fork())
                .build(),
        ]);
        assert_eq!(
            stars(&m),
            [
                row(
                    "vendor::lib::a::f",
                    "lib::a::f",
                    "body_hash",
                    Confidence::Weak
                ),
                row(
                    "vendor::lib::a::g",
                    "lib::a::g",
                    "path_suffix/diverged",
                    Confidence::Weak
                ),
            ]
        );
        // One offset (``, `vendor`), confirmed by f; the MODULEs (no CODE)
        // are pairs of it that emit nothing.
        assert_eq!(
            identity_edges(&m.graphs, &[]).1.line().as_deref(),
            Some(
                "[identity] classes=2 edges=2 diverged=1 package_name=0 manifest_name=0 \
                 message_fqname=0 contract_op=0 contract_route=0 scip_symbol=0 body_hash=1 path_suffix=1 \
                 offsets=1/1"
            )
        );
    }

    #[test]
    fn unconfirmed_offset() {
        // The boilerplate trap: one path in two unrelated repos, no identical
        // declaration under the offset.
        let graphs = vec![
            G::new("web")
                .func(
                    "src/index.ts",
                    "main",
                    body(60, "export function main() { bootWeb(); }"),
                )
                .build(),
            G::new("admin")
                .func(
                    "src/index.ts",
                    "main",
                    body(60, "export function main() { bootAdmin(); }"),
                )
                .build(),
        ];
        let (edges, report) = identity_edges(&graphs, &[]);
        assert!(edges.is_empty(), "{edges:?}");
        assert_eq!((report.offsets_confirmed, report.offsets_seen), (0, 1));
        assert_eq!(report.line(), None);
    }

    #[test]
    fn module_match_confirms_nothing() {
        // A byte-identical declaration-free file (a scaffold's `main.ts`) at
        // one path in two apps whose same-path pages differ: no offset.
        let boot = || body(60, "bootstrapApplication(AppComponent, appConfig);");
        let app = |tag: &str, page: &str| {
            let mut g = G::new(tag).func("src/pages/login.ts", "login", body(60, page));
            let pos = json(
                cell_type::POSITION,
                r#"{"file":"src/main.ts","start_line":0}"#,
            );
            let code = Cell {
                kind: cell_type::CODE,
                payload: boot(),
            };
            g.add(node_kind::MODULE, "src::main", "main", vec![code, pos]);
            g.build()
        };
        let graphs = vec![
            app("kina", "export function login() { return swapLogin(); }"),
            app(
                "lapse",
                "export function login() { return registerLogin(); }",
            ),
        ];
        let (edges, report) = identity_edges(&graphs, &[]);
        assert!(edges.is_empty(), "{edges:?}");
        assert_eq!((report.offsets_confirmed, report.offsets_seen), (0, 1));
    }

    #[test]
    fn short_bodies() {
        // Equal 20-byte bodies have no digest, so they confirm nothing, and
        // the diverged sibling under the same offset pairs nothing either.
        let short = || body(20, "function f() {}");
        let m = run(vec![
            G::new("lib")
                .func("lib/a.ts", "f", short())
                .func("lib/a.ts", "g", g_lib())
                .build(),
            G::new("app")
                .func("vendor/lib/a.ts", "f", short())
                .func("vendor/lib/a.ts", "g", g_fork())
                .build(),
        ]);
        assert!(stars(&m).is_empty(), "{:?}", stars(&m));
    }

    #[test]
    fn span_and_text_agree() {
        // A layout merged without its sources reads CODE back as a CodeSpan:
        // the digest is the span's xxh64 of the same bytes.
        let span = |file: &str, p: CellPayload| match p {
            CellPayload::Text(t) => CodeSpan::of(file, 7, t.as_bytes()).to_payload(),
            other => other,
        };
        let build = |spans: bool| {
            let (f, g) = if spans {
                (
                    span("vendor/lib/a.ts", f60()),
                    span("vendor/lib/a.ts", g_fork()),
                )
            } else {
                (f60(), g_fork())
            };
            run(vec![
                G::new("lib")
                    .func("lib/a.ts", "f", f60())
                    .func("lib/a.ts", "g", g_lib())
                    .build(),
                G::new("app")
                    .func("vendor/lib/a.ts", "f", f)
                    .func("vendor/lib/a.ts", "g", g)
                    .build(),
            ])
        };
        let (text, spans) = (build(false), build(true));
        assert_eq!(stars(&text).len(), 2);
        assert_eq!(stars(&text), stars(&spans));
    }

    #[test]
    fn intra_repo() {
        // Two copies inside one repo are that repo's business; the other repo
        // holds no copy.
        let m = run(vec![
            G::new("app")
                .func("lib/a.ts", "f", f60())
                .func("vendor/lib/a.ts", "f", f60())
                .build(),
            G::new("other").dep("package:npm:react").build(),
        ]);
        assert!(stars(&m).is_empty(), "{:?}", stars(&m));
        // And a one-repo build never indexes a declaration at all.
        let solo = vec![
            G::new("app")
                .func("lib/a.ts", "f", f60())
                .func("vendor/lib/a.ts", "f", f60())
                .build(),
        ];
        assert_eq!(identity_edges(&solo, &[]).1.offsets_seen, 0);
    }

    #[test]
    fn three_repos() {
        let m = run(vec![
            G::new("lib").func("lib/a.ts", "f", f60()).build(),
            G::new("app").func("vendor/lib/a.ts", "f", f60()).build(),
            G::new("cli")
                .func("third_party/lib/a.ts", "f", f60())
                .build(),
        ]);
        assert_eq!(
            stars(&m),
            [
                row(
                    "third_party::lib::a::f",
                    "lib::a::f",
                    "body_hash",
                    Confidence::Weak
                ),
                row(
                    "vendor::lib::a::f",
                    "lib::a::f",
                    "body_hash",
                    Confidence::Weak
                ),
            ]
        );
        let report = identity_edges(&m.graphs, &[]).1;
        assert_eq!(report.classes[6], 1, "one class, not two");
        // lib-app and lib-cli; app's and cli's paths are no suffix of each other.
        assert_eq!((report.offsets_confirmed, report.offsets_seen), (2, 2));
    }

    #[test]
    fn ambiguous_key_pairs_only_identical() {
        // The library's a.ts holds two FUNCTIONs named `g` (distinct qnames,
        // one key): a copy's `g` pairs only with the one it is byte-identical
        // to, never as a diverged copy of either.
        let other = || body(60, "export function g(s: string) { return s.padStart(8); }");
        let lib = |second_g: CellPayload| {
            let mut g = G::new("lib")
                .func("lib/a.ts", "f", f60())
                .func("lib/a.ts", "g", g_lib());
            let module = g.modules["lib/a.ts"];
            let pos = json(cell_type::POSITION, r#"{"file":"lib/a.ts","start_line":9}"#);
            let code = Cell {
                kind: cell_type::CODE,
                payload: second_g,
            };
            g.add_under(
                node_kind::FUNCTION,
                "lib::a::g#2",
                "g",
                vec![code, pos],
                Some(module),
            );
            g.build()
        };
        let app = |g_body: CellPayload| {
            G::new("app")
                .func("vendor/lib/a.ts", "f", f60())
                .func("vendor/lib/a.ts", "g", g_body)
                .build()
        };
        let f_row = || {
            row(
                "vendor::lib::a::f",
                "lib::a::f",
                "body_hash",
                Confidence::Weak,
            )
        };
        // The copy's g differs from both: no path_suffix pair at all.
        assert_eq!(stars(&run(vec![lib(other()), app(g_fork())])), [f_row()]);
        // The copy's g is the second one byte for byte: that one pair only.
        assert_eq!(
            stars(&run(vec![lib(g_fork()), app(g_fork())])),
            [
                f_row(),
                row(
                    "vendor::lib::a::g",
                    "lib::a::g#2",
                    "body_hash",
                    Confidence::Weak
                ),
            ]
        );
    }

    #[test]
    fn local_chain_bounds() {
        let mut g = G::new("a");
        let module = g.add(node_kind::MODULE, "m", "m", vec![]);
        let class = g.add_under(node_kind::CLASS, "m::C", "C", vec![], Some(module));
        let method = g.add_under(node_kind::METHOD, "m::C::run", "run", vec![], Some(class));
        let orphan = g.add(node_kind::FUNCTION, "x::f", "f", vec![]);
        // A parent cycle with no MODULE on it.
        let p = g.add(node_kind::CLASS, "y::P", "P", vec![]);
        let q = g.add_under(node_kind::METHOD, "y::P::q", "q", vec![], Some(p));
        g.nav.parent_of.insert(p, q);
        let g = g.build();
        assert_eq!(local_chain(&g, module).as_deref(), Some(""));
        assert_eq!(local_chain(&g, method).as_deref(), Some("C::run"));
        assert_eq!(local_chain(&g, orphan), None);
        assert_eq!(local_chain(&g, q), None);
    }

    #[test]
    fn suffix_cuts() {
        let all: Vec<(&str, &str)> = suffixes("a/é/b.ts", true).collect();
        assert_eq!(all, [("", "a/é/b.ts"), ("a", "é/b.ts"), ("a/é", "b.ts")]);
        let proper: Vec<(&str, &str)> = suffixes("b.ts", false).collect();
        assert!(proper.is_empty());
        assert_eq!(suffixes("dir/", false).count(), 0, "no empty suffix");
    }

    // ---- SCIP symbols (DD.7) --------------------------------------------------

    const MONEY_10: &str = "scip-python python money 1.0 `lib.money`/format_cents().";
    const MONEY_11: &str = "scip-python python money 1.1 `lib.money`/format_cents().";
    const PARSE_11: &str = "scip-python python money 1.1 `lib.money`/parse_cents().";
    const BILLING: &str = "billing::money::format_cents";
    const SHOP: &str = "shop::pay::format_cents";

    /// `graphs` resolved with the SCIP keys `(graph index, qname, symbol)`.
    fn run_scip(graphs: Vec<RepoGraph>, keys: &[(usize, &str, &str)]) -> MergedGraph {
        let mut m = MergedGraph::new(graphs);
        let scip: Vec<(NodeId, String)> = keys
            .iter()
            .map(|&(g, q, sym)| (id_of(&m, g, q), sym.to_string()))
            .collect();
        m.run(&IdentityResolver::with_scip(&scip));
        m
    }

    /// Two repos' `format_cents`, in files of different basenames so the
    /// vendored rules never pair them: only a SCIP key can.
    fn two_defs(shop_body: CellPayload) -> Vec<RepoGraph> {
        vec![
            G::new("billing")
                .func("billing/money.ts", "format_cents", f60())
                .build(),
            G::new("shop")
                .func("shop/pay.ts", "format_cents", shop_body)
                .build(),
        ]
    }

    #[test]
    fn scip_rule_groups_by_key() {
        // The version is no part of the key: one identity.
        let m = run_scip(
            two_defs(f60()),
            &[(0, BILLING, MONEY_10), (1, SHOP, MONEY_11)],
        );
        assert_eq!(
            stars(&m),
            [row(SHOP, BILLING, "scip_symbol", Confidence::Strong)]
        );
        assert_eq!(
            identity_edges(
                &m.graphs,
                &[
                    (id_of(&m, 0, BILLING), MONEY_10.to_string()),
                    (id_of(&m, 1, SHOP), MONEY_11.to_string()),
                ]
            )
            .1
            .line()
            .as_deref(),
            Some(
                "[identity] classes=1 edges=1 diverged=0 package_name=0 manifest_name=0 \
                 message_fqname=0 contract_op=0 contract_route=0 scip_symbol=1 body_hash=0 \
                 path_suffix=0 offsets=0/0"
            )
        );
        // Another descriptor is another identity.
        let apart = run_scip(
            two_defs(f60()),
            &[(0, BILLING, MONEY_10), (1, SHOP, PARSE_11)],
        );
        assert!(stars(&apart).is_empty(), "{:?}", stars(&apart));
        // No keys, no class: the same graphs under `new()`.
        assert!(stars(&run(two_defs(f60()))).is_empty());
    }

    #[test]
    fn scip_diverged_bodies() {
        let m = run_scip(
            two_defs(g_lib()),
            &[(0, BILLING, MONEY_10), (1, SHOP, MONEY_11)],
        );
        assert_eq!(
            stars(&m),
            [row(
                SHOP,
                BILLING,
                "scip_symbol/diverged",
                Confidence::Strong
            )]
        );
    }

    #[test]
    fn scip_keys_need_global_symbols_of_rule_kinds() {
        // `local` symbols are file-scoped: never an identity.
        let local = run_scip(
            two_defs(f60()),
            &[(0, BILLING, "local 0"), (1, SHOP, "local 0")],
        );
        assert!(stars(&local).is_empty(), "{:?}", stars(&local));
        // A MODULE is no definition the SCIP rule pairs.
        let modules = run_scip(
            two_defs(f60()),
            &[(0, "billing::money", MONEY_10), (1, "shop::pay", MONEY_10)],
        );
        assert!(stars(&modules).is_empty(), "{:?}", stars(&modules));
        // One repo alone: a key with one node is no class.
        let one = run_scip(two_defs(f60()), &[(0, BILLING, MONEY_10)]);
        assert!(stars(&one).is_empty(), "{:?}", stars(&one));
    }

    #[test]
    fn scip_aliases_write_one_edge() {
        // The pair keyed by two symbols (an alias the index defines twice) is
        // one class: no edge is written twice.
        let alias = "scip-python python money 1.0 `lib`/format_cents().";
        let m = run_scip(
            two_defs(f60()),
            &[
                (0, BILLING, MONEY_10),
                (1, SHOP, MONEY_11),
                (0, BILLING, alias),
                (1, SHOP, alias),
            ],
        );
        assert_eq!(
            stars(&m),
            [row(SHOP, BILLING, "scip_symbol", Confidence::Strong)]
        );
    }

    /// The evidence-rules tally counts edges per rule, the `[identity]` line
    /// classes per rule; both are silent with nothing to pair.
    #[test]
    fn markers() {
        let m = run(vec![G::new("solo").dep("package:npm:react").build()]);
        let report = identity_edges(&m.graphs, &[]).1;
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
        let report = identity_edges(&graphs, &[]).1;
        assert_eq!(
            report.tally.line().as_deref(),
            Some(
                "[evidence-rules] resolver=identity package_name=3 manifest_name=0 message_fqname=0 \
                 contract_op=0 contract_route=0 scip_symbol=0 body_hash=0 path_suffix=0"
            )
        );
        assert_eq!(
            report.line().as_deref(),
            Some(
                "[identity] classes=2 edges=3 diverged=0 package_name=2 manifest_name=0 \
                 message_fqname=0 contract_op=0 contract_route=0 scip_symbol=0 body_hash=0 path_suffix=0 \
                 offsets=0/0"
            )
        );
    }
}
