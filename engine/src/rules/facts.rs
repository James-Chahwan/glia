//! `CodeFacts`: the code domain's [`FactSource`] over a [`MergedGraph`] (DB.2a).
//!
//! **Nodes.** One node per id: [`GraphSource::node_ids`] is the merged graph's node
//! order (graphs in `Vec` order, each graph's node list in order) with every later
//! occurrence of an id dropped, since an id two language graphs share is listed by
//! both (check's `Ctx::new` and the Locator keep the first the same way). A node's
//! cells come from the first graph whose node list holds it; its kind, name, qname
//! and parent from the first graph whose nav names it, the [`Locator`]'s rule, so
//! [`FactSource::render_node`] (the qname, the checkout-stable key rows sort by) is
//! exactly `Locator::locate(id).qname`, `(unknown:<id>)` for an id no graph names.
//! Edges are [`MergedGraph::all_edges`] order.
//!
//! **Attributes** ([`CODE_ATTRIBUTES`]), each read only when a program names it, in
//! node order: `qname` / `name` (nav), `file` and the 1-based `line` (the Locator;
//! an unlocated node has no row), `repo` (`repo#<k>`, `k` the position of the node's
//! RepoId among the distinct RepoIds of the merge in order: a RepoId hashes a path,
//! so its bits differ between checkouts and the merge-order label does not),
//! `project` (the label of every PROJECT of the node's repo at the longest path its
//! file sits under; a PROJECT's own label), `parent` (nav), `role`
//! (`graph::roles::roles_in`, by kind name) and `origin` (an ORIGIN cell's
//! `provenance`: `external`, `test_fixture`, `nav_route`, ...). Every column's first
//! load prints `[rules] facts attr=<name> rows=<n>` once per `CodeFacts` and is kept
//! in [`CodeFacts::loaded_attributes`].
//!
//! **Scopes.** [`CodeFacts::members`] is check's strict membership, literally: a node
//! is in scope `X` when the file the Locator places it in sits under `X` on a
//! segment boundary (`answers::in_scope`), or it is a PROJECT whose path does; an
//! unlocated node is in no scope, and the path is never label-resolved (check's
//! scopes arrive resolved). The `in_scope(N, Scope)` builtin, the user-facing
//! form, resolves `Scope` through `answers::resolve_scope` once per distinct string
//! first, so a project label or a `project:<path>` qname works in a rule as every
//! `--scope` flag accepts it. `under(Path, Prefix)` is the same segment test over
//! two strings. The file column behind both is built once, on first use, and is
//! not the `file` attribute: a program that only scopes never loads `file`.
//!
//! **Tiers.** An edge's tier is `why::tier_of` (emitter stage, name-only rules,
//! confidence, ORIGIN): one rule for `why`, `check` and the rules layer.
//!
//! Every output order comes from the node order or `all_edges`; the maps here are
//! looked up only. The two nav passes in [`CodeFacts::new`] insert first-wins per
//! graph in graph order, so their result does not depend on a map's iteration.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use glia_activation::algo::GraphSource;
use glia_activation::profile::DomainTables;
use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{Cell, CellPayload, Edge, EdgeCategoryId, NodeId, NodeKindId, RepoId};
use glia_graph::roles::roles_in;
use glia_graph::{MergedGraph, RepoGraph};
use glia_rules::ir::{AttrDecl, BindReq, BuiltinDecl, BuiltinKind, ColType};
use glia_rules::source::{Builtin, BuiltinRow, FactSource};
use glia_rules::value::{Tier, Value, ValueRef};

use crate::answers::{Locator, in_scope, project_roots, resolve_scope};
use crate::check::{self, ViolationEdge};
use crate::profile::CODE_PROFILE;
use crate::why;

/// The code domain's attribute columns, each the predicate `name(Node, ty)`
/// (module doc).
pub const CODE_ATTRIBUTES: &[AttrDecl] = &[
    attr("qname", ColType::Str),
    attr("name", ColType::Str),
    attr("file", ColType::Str),
    attr("line", ColType::Int),
    attr("repo", ColType::Str),
    attr("project", ColType::Str),
    attr("parent", ColType::Node),
    attr("role", ColType::Str),
    attr("origin", ColType::Str),
];

const fn attr(name: &'static str, ty: ColType) -> AttrDecl {
    AttrDecl { name, ty }
}

/// The code domain's [`FactSource`] over one [`MergedGraph`] (module doc).
///
/// Single-threaded, like the evaluator: the lazily built columns live behind
/// `OnceCell` / `RefCell`. Build one per answer and share it across the programs
/// that answer evaluates; [`CodeFacts::with_locator`] reuses the caller's Locator
/// instead of building a second O(V) index.
pub struct CodeFacts<'a> {
    inner: Rc<Inner<'a>>,
    in_scope: InScope<'a>,
    under: Under,
    /// The attribute columns loaded, in load order, each once.
    loaded: RefCell<Vec<&'static str>>,
}

/// The state the facts and the `in_scope` builtin share.
struct Inner<'a> {
    merged: &'a MergedGraph,
    loc: Loc<'a>,
    /// Every node id once, at its first occurrence in node order.
    order: Vec<NodeId>,
    at: HashMap<NodeId, At>,
    /// Per graph of the merge: its RepoId's position among the distinct RepoIds.
    repo_index: Vec<usize>,
    /// Every node's located file (`Locator::file_of`), built on first use.
    files: OnceCell<HashMap<NodeId, Option<String>>>,
    /// PROJECT node -> its repo-relative path (`.` = the root), built on first use.
    projects: OnceCell<HashMap<NodeId, String>>,
    /// Literal scope -> its members.
    scopes: RefCell<BTreeMap<String, Rc<HashSet<NodeId>>>>,
    /// `in_scope`'s scope string -> the path `resolve_scope` gives it.
    resolved: RefCell<BTreeMap<String, String>>,
}

/// Where an id lives.
#[derive(Clone, Copy, Default)]
struct At {
    /// `(graph, node index)`: the first graph whose node list holds the id.
    node: Option<(usize, usize)>,
    /// The first graph whose nav names the id.
    nav: Option<usize>,
}

/// The Locator a `CodeFacts` reads: its own, or the caller's.
enum Loc<'a> {
    Owned(Locator<'a>),
    Borrowed(&'a Locator<'a>),
}

impl<'a> Loc<'a> {
    fn get(&self) -> &Locator<'a> {
        match self {
            Loc::Owned(l) => l,
            Loc::Borrowed(l) => l,
        }
    }
}

impl<'a> CodeFacts<'a> {
    /// Facts over `merged`, with a Locator of their own.
    pub fn new(merged: &'a MergedGraph) -> Self {
        Self::build(merged, Loc::Owned(Locator::new(merged)))
    }

    /// Facts over `merged` reading `loc`, which must be a Locator over `merged`:
    /// a caller that already holds one (check) shares it.
    pub fn with_locator(merged: &'a MergedGraph, loc: &'a Locator<'a>) -> Self {
        Self::build(merged, Loc::Borrowed(loc))
    }

    fn build(merged: &'a MergedGraph, loc: Loc<'a>) -> Self {
        let mut at: HashMap<NodeId, At> = HashMap::new();
        let mut order = Vec::new();
        for (gi, g) in merged.graphs.iter().enumerate() {
            for (ni, n) in g.nodes.iter().enumerate() {
                let slot = at.entry(n.id).or_default();
                if slot.node.is_none() {
                    slot.node = Some((gi, ni));
                    order.push(n.id);
                }
            }
        }
        for (gi, g) in merged.graphs.iter().enumerate() {
            for id in g.nav.qname_by_id.keys() {
                let slot = at.entry(*id).or_default();
                if slot.nav.is_none() {
                    slot.nav = Some(gi);
                }
            }
        }
        let mut repos: Vec<RepoId> = Vec::new();
        let repo_index = merged
            .graphs
            .iter()
            .map(|g| match repos.iter().position(|r| *r == g.repo) {
                Some(k) => k,
                None => {
                    repos.push(g.repo);
                    repos.len() - 1
                }
            })
            .collect();
        let inner = Rc::new(Inner {
            merged,
            loc,
            order,
            at,
            repo_index,
            files: OnceCell::new(),
            projects: OnceCell::new(),
            scopes: RefCell::new(BTreeMap::new()),
            resolved: RefCell::new(BTreeMap::new()),
        });
        CodeFacts {
            in_scope: InScope {
                inner: Rc::clone(&inner),
            },
            under: Under,
            inner,
            loaded: RefCell::new(Vec::new()),
        }
    }

    /// The Locator every file / line here comes from.
    pub fn locator(&self) -> &Locator<'a> {
        self.inner.loc.get()
    }

    /// The nodes strictly in `scope`, check's membership exactly (module doc): no
    /// label resolution. Memoised per scope string.
    pub fn members(&self, scope: &str) -> Rc<HashSet<NodeId>> {
        self.inner.members(scope)
    }

    /// The attribute columns loaded so far, in load order, each once.
    pub fn loaded_attributes(&self) -> Vec<&'static str> {
        self.loaded.borrow().clone()
    }

    /// The located, tiered row of `e`, as check reports an evidence row: at its
    /// EVIDENCE site when that names a file, else at `from`'s location; the tier
    /// and note `why` gives the edge.
    pub fn locate_edge(&self, e: &Edge) -> ViolationEdge {
        let ev = Evidence::of(e);
        let (tier, note) = tier_and_note(ev.as_ref(), e);
        located_edge(
            self.locator(),
            e.from,
            e.category,
            e.to,
            ev.as_ref(),
            (tier_name(tier), note),
        )
    }

    /// Every node's located file (built once).
    pub(crate) fn files(&self) -> &HashMap<NodeId, Option<String>> {
        self.inner.files()
    }

    /// PROJECT node -> its repo-relative path (built once).
    pub(crate) fn projects(&self) -> &HashMap<NodeId, String> {
        self.inner.projects()
    }

    /// The graph whose nav names `id` first.
    fn nav_graph(&self, id: NodeId) -> Option<&'a RepoGraph> {
        self.inner.nav_graph(id)
    }

    /// One attribute column, in node order (module doc).
    fn column(&self, name: &str, out: &mut Vec<(NodeId, Value)>) {
        let inner = &*self.inner;
        match name {
            "qname" | "name" | "parent" => {
                for &id in &inner.order {
                    let Some(nav) = self.nav_graph(id).map(|g| &g.nav) else {
                        continue;
                    };
                    let v = match name {
                        "qname" => nav.qname_by_id.get(&id).cloned().map(Value::Str),
                        "name" => nav.name_by_id.get(&id).cloned().map(Value::Str),
                        _ => nav.parent_of.get(&id).copied().map(Value::Node),
                    };
                    out.extend(v.map(|v| (id, v)));
                }
            }
            "file" => {
                let files = self.files();
                for &id in &inner.order {
                    if let Some(Some(f)) = files.get(&id) {
                        out.push((id, Value::Str(f.clone())));
                    }
                }
            }
            "line" => {
                let loc = self.locator();
                for &id in &inner.order {
                    out.extend(loc.locate(id).line.map(|l| (id, Value::Int(l))));
                }
            }
            "repo" => {
                for &id in &inner.order {
                    if let Some(gi) = inner.node_graph(id) {
                        let k = inner.repo_index.get(gi).copied().unwrap_or(gi);
                        out.push((id, Value::Str(format!("repo#{k}"))));
                    }
                }
            }
            "project" => self.project_column(out),
            "role" => {
                for &id in &inner.order {
                    for k in roles_in(self.node_kind(id), self.node_cells(id)) {
                        out.push((id, Value::Str(node_kind::name(k).to_string())));
                    }
                }
            }
            "origin" => {
                for &id in &inner.order {
                    for p in self.node_cells(id).iter().filter_map(provenance) {
                        out.push((id, Value::Str(p)));
                    }
                }
            }
            _ => {}
        }
    }

    /// `project/2`: a PROJECT's own label; a located node's, every PROJECT of its
    /// repo at the longest path its file sits under (two manifests in one
    /// directory are two projects there).
    fn project_column(&self, out: &mut Vec<(NodeId, Value)>) {
        let inner = &*self.inner;
        let paths = self.projects();
        // (repo, normalised path, label) of every PROJECT, in node order.
        let mut anchors: Vec<(usize, &str, String)> = Vec::new();
        let mut own: HashMap<NodeId, usize> = HashMap::new();
        for &id in &inner.order {
            let (Some(path), Some((gi, ni))) = (paths.get(&id), inner.node_of(id)) else {
                continue;
            };
            let g = &inner.merged.graphs[gi];
            own.insert(id, anchors.len());
            anchors.push((
                inner.repo_index.get(gi).copied().unwrap_or(gi),
                norm_path(path),
                project_label(g, &g.nodes[ni].cells, id),
            ));
        }
        let files = self.files();
        for &id in &inner.order {
            if let Some(&i) = own.get(&id) {
                out.push((id, Value::Str(anchors[i].2.clone())));
                continue;
            }
            let (Some(Some(file)), Some(gi)) = (files.get(&id), inner.node_graph(id)) else {
                continue;
            };
            let repo = inner.repo_index.get(gi).copied().unwrap_or(gi);
            let depth = |p: &str| if p == "." { 0 } else { p.len() };
            let held = anchors
                .iter()
                .filter(|(r, p, _)| *r == repo && in_scope(file, p));
            let Some(best) = held.clone().map(|(_, p, _)| depth(p)).max() else {
                continue;
            };
            for (_, _, label) in held.filter(|(_, p, _)| depth(p) == best) {
                out.push((id, Value::Str(label.clone())));
            }
        }
    }
}

impl<'a> Inner<'a> {
    fn nav_graph(&self, id: NodeId) -> Option<&'a RepoGraph> {
        let gi = self.at.get(&id)?.nav?;
        self.merged.graphs.get(gi)
    }

    fn node_of(&self, id: NodeId) -> Option<(usize, usize)> {
        self.at.get(&id)?.node
    }

    fn node_graph(&self, id: NodeId) -> Option<usize> {
        self.node_of(id).map(|(gi, _)| gi)
    }

    fn files(&self) -> &HashMap<NodeId, Option<String>> {
        self.files.get_or_init(|| {
            let loc = self.loc.get();
            self.order.iter().map(|&id| (id, loc.file_of(id))).collect()
        })
    }

    /// check's PROJECT paths: a node whose first occurrence's graph names it a
    /// PROJECT, with the path `project_roots` gives its qname.
    fn projects(&self) -> &HashMap<NodeId, String> {
        self.projects.get_or_init(|| {
            let path_of: HashMap<String, String> = project_roots(self.merged)
                .into_iter()
                .map(|p| (p.qname, p.path))
                .collect();
            let mut out = HashMap::new();
            for &id in &self.order {
                let Some((gi, _)) = self.node_of(id) else {
                    continue;
                };
                let nav = &self.merged.graphs[gi].nav;
                if nav.kind_by_id.get(&id) == Some(&node_kind::PROJECT)
                    && let Some(p) = nav.qname_by_id.get(&id).and_then(|q| path_of.get(q))
                {
                    out.insert(id, p.clone());
                }
            }
            out
        })
    }

    fn members(&self, scope: &str) -> Rc<HashSet<NodeId>> {
        if let Some(set) = self.scopes.borrow().get(scope) {
            return Rc::clone(set);
        }
        let (files, projects) = (self.files(), self.projects());
        let set: HashSet<NodeId> = self
            .order
            .iter()
            .copied()
            .filter(|id| {
                let located = files
                    .get(id)
                    .and_then(Option::as_deref)
                    .is_some_and(|f| in_scope(f, scope));
                located || projects.get(id).is_some_and(|p| in_scope(p, scope))
            })
            .collect();
        let set = Rc::new(set);
        self.scopes
            .borrow_mut()
            .insert(scope.to_string(), Rc::clone(&set));
        set
    }

    /// The members of a user-supplied scope: label / qname resolved once per string.
    fn resolved_members(&self, scope: &str) -> Rc<HashSet<NodeId>> {
        let known = self.resolved.borrow().get(scope).cloned();
        let path = match known {
            Some(p) => p,
            None => {
                let p = resolve_scope(self.merged, scope);
                self.resolved
                    .borrow_mut()
                    .insert(scope.to_string(), p.clone());
                p
            }
        };
        self.members(&path)
    }
}

impl GraphSource for CodeFacts<'_> {
    fn node_ids(&self) -> Vec<NodeId> {
        self.inner.order.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.inner.merged.all_edges())
    }
}

impl FactSource for CodeFacts<'_> {
    fn tables(&self) -> &DomainTables {
        &CODE_PROFILE.tables
    }

    fn node_kind(&self, id: NodeId) -> Option<NodeKindId> {
        self.nav_graph(id)?.nav.kind_by_id.get(&id).copied()
    }

    fn node_cells(&self, id: NodeId) -> &[Cell] {
        match self.inner.node_of(id) {
            Some((gi, ni)) => &self.inner.merged.graphs[gi].nodes[ni].cells,
            None => &[],
        }
    }

    fn render_node(&self, id: NodeId) -> String {
        self.nav_graph(id)
            .and_then(|g| g.nav.qname_by_id.get(&id))
            .cloned()
            .unwrap_or_else(|| format!("(unknown:{})", id.0))
    }

    fn attributes(&self) -> &[AttrDecl] {
        CODE_ATTRIBUTES
    }

    fn attr_rows(&self, attr: usize, out: &mut Vec<(NodeId, Value)>) {
        let Some(decl) = CODE_ATTRIBUTES.get(attr) else {
            return;
        };
        let before = out.len();
        self.column(decl.name, out);
        let mut loaded = self.loaded.borrow_mut();
        if !loaded.contains(&decl.name) {
            loaded.push(decl.name);
            eprintln!(
                "[rules] facts attr={} rows={}",
                decl.name,
                out.len() - before
            );
        }
    }

    fn edge_tier(&self, _ordinal: usize, e: &Edge) -> Tier {
        edge_tier_of(e)
    }

    fn builtins(&self) -> Vec<&dyn Builtin> {
        vec![&self.in_scope, &self.under]
    }
}

// ============================================================================
// Tiers and located edges
// ============================================================================

/// The word `why` and `check` print for `t`.
pub(crate) fn tier_name(t: Tier) -> &'static str {
    match t {
        Tier::Fact => check::FACT,
        Tier::Derived => check::DERIVED,
        Tier::Heuristic => check::HEURISTIC,
    }
}

/// `why`'s word as a [`Tier`]; anything else is DERIVED, `why`'s own fallback.
fn tier_of_name(word: &str) -> Tier {
    match word {
        why::FACT => Tier::Fact,
        why::HEURISTIC => Tier::Heuristic,
        _ => Tier::Derived,
    }
}

/// `why::tier_of` over `e` and its evidence, as a [`Tier`] and the note.
fn tier_and_note(ev: Option<&Evidence>, e: &Edge) -> (Tier, Option<String>) {
    let (word, note) = why::tier_of(ev, e);
    (tier_of_name(word), note)
}

/// The tier `why` gives `e`: one rule for `why`, `check` and the rules layer.
pub(crate) fn edge_tier_of(e: &Edge) -> Tier {
    tier_and_note(Evidence::of(e).as_ref(), e).0
}

/// One located evidence row: at the edge's EVIDENCE site (its 0-based line + 1)
/// when it names a file, else at `from`'s Locator location. `(tier, note)` is
/// `why::tier_of` over the edge, computed by the caller that holds it.
pub(crate) fn located_edge(
    loc: &Locator<'_>,
    from: NodeId,
    category: EdgeCategoryId,
    to: NodeId,
    ev: Option<&Evidence>,
    (tier, note): (&'static str, Option<String>),
) -> ViolationEdge {
    let f = loc.locate(from);
    let site = ev.and_then(|e| {
        e.file
            .clone()
            .map(|file| (Some(file), e.line.map(|l| i64::from(l) + 1)))
    });
    let (file, line) = site.unwrap_or((f.file, f.line));
    ViolationEdge {
        from_qname: f.qname,
        to_qname: loc.locate(to).qname,
        category: edge_category::name(category),
        file,
        line,
        emitter: ev.map(|e| e.emitter.clone()),
        tier,
        note,
    }
}

// ============================================================================
// Cells
// ============================================================================

/// A cell's JSON object, when it is one.
fn json_object(c: &Cell) -> Option<serde_json::Value> {
    let (CellPayload::Json(s) | CellPayload::Text(s)) = &c.payload else {
        return None;
    };
    serde_json::from_str::<serde_json::Value>(s)
        .ok()
        .filter(serde_json::Value::is_object)
}

/// The `provenance` of an ORIGIN cell.
fn provenance(c: &Cell) -> Option<String> {
    if c.kind != cell_type::ORIGIN {
        return None;
    }
    json_object(c)?
        .get("provenance")?
        .as_str()
        .map(str::to_string)
}

/// A PROJECT's label, by `project_roots`' rule: its ORIGIN cell's `label`, else
/// its nav name.
fn project_label(g: &RepoGraph, cells: &[Cell], id: NodeId) -> String {
    cells
        .iter()
        .filter(|c| c.kind == cell_type::ORIGIN)
        .find_map(json_object)
        .and_then(|v| {
            v.get("label")
                .and_then(serde_json::Value::as_str)
                .map(String::from)
        })
        .or_else(|| g.nav.name_by_id.get(&id).cloned())
        .unwrap_or_default()
}

// ============================================================================
// The builtins
// ============================================================================

/// A path as scope membership compares it (check's `norm_path`): no leading `./`
/// or `/`, no trailing `/`; the root is `.`.
fn norm_path(p: &str) -> &str {
    let s = p
        .trim_start_matches("./")
        .trim_start_matches('/')
        .trim_end_matches('/');
    if s.is_empty() { "." } else { s }
}

/// `path` sits under `prefix` on a segment boundary; the root holds every path.
fn under(path: &str, prefix: &str) -> bool {
    let (p, s) = (norm_path(path), norm_path(prefix));
    s == "." || p == s || p.strip_prefix(s).is_some_and(|rest| rest.starts_with('/'))
}

/// `in_scope(N, Scope)`: `N` (bound or free) is in `Scope` (bound).
struct InScope<'a> {
    inner: Rc<Inner<'a>>,
}

impl Builtin for InScope<'_> {
    fn decl(&self) -> BuiltinDecl {
        BuiltinDecl {
            name: "in_scope".to_string(),
            cols: vec![ColType::Node, ColType::Str],
            bind: vec![BindReq::May, BindReq::Must],
            tiered: false,
            kind: BuiltinKind::Domain,
        }
    }

    fn call(&self, args: &[Option<ValueRef<'_>>], out: &mut Vec<BuiltinRow>) -> Result<(), String> {
        let [node, scope] = args else {
            return Err(format!("takes 2 arguments, got {}", args.len()));
        };
        let Some(scope) = scope else {
            return Err("argument 2 (the scope) is unbound".to_string());
        };
        // A non-string scope names no path: nothing sits in it.
        let ValueRef::Str(scope) = scope else {
            return Ok(());
        };
        let members = self.inner.resolved_members(scope);
        let row = |id: NodeId| BuiltinRow {
            values: vec![Value::Node(id), Value::Str((*scope).to_string())],
            tier: Tier::Fact,
        };
        match node {
            Some(ValueRef::Node(id)) => {
                if members.contains(id) {
                    out.push(row(*id));
                }
            }
            Some(_) => {}
            None => {
                out.extend(
                    self.inner
                        .order
                        .iter()
                        .filter(|id| members.contains(*id))
                        .map(|&id| row(id)),
                );
            }
        }
        Ok(())
    }
}

/// `under(Path, Prefix)`: both strings bound.
struct Under;

impl Builtin for Under {
    fn decl(&self) -> BuiltinDecl {
        BuiltinDecl {
            name: "under".to_string(),
            cols: vec![ColType::Str, ColType::Str],
            bind: vec![BindReq::Must, BindReq::Must],
            tiered: false,
            kind: BuiltinKind::Domain,
        }
    }

    fn call(&self, args: &[Option<ValueRef<'_>>], out: &mut Vec<BuiltinRow>) -> Result<(), String> {
        let [a, b] = args else {
            return Err(format!("takes 2 arguments, got {}", args.len()));
        };
        let (Some(a), Some(b)) = (a, b) else {
            let n = if a.is_none() { 1 } else { 2 };
            return Err(format!("argument {n} is unbound"));
        };
        if let (ValueRef::Str(path), ValueRef::Str(prefix)) = (a, b)
            && under(path, prefix)
        {
            out.push(BuiltinRow {
                values: vec![
                    Value::Str((*path).to_string()),
                    Value::Str((*prefix).to_string()),
                ],
                tier: Tier::Fact,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tier_words_round_trip_and_match_the_rules_crate() {
        for t in Tier::ALL {
            assert_eq!(tier_of_name(tier_name(t)), t);
            assert_eq!(tier_name(t), t.name(), "check's word is glia_rules' word");
        }
        assert_eq!(tier_of_name("no such tier"), Tier::Derived);
    }

    #[test]
    fn under_normalises_like_check() {
        assert_eq!(norm_path("./web/"), "web");
        assert_eq!(norm_path("/"), ".");
        assert_eq!(norm_path(""), ".");
        assert!(under("web/app.py", "web"));
        assert!(!under("webhooks/a.py", "web"));
        assert!(under("x/y.py", "."));
        assert!(under(".", ""));
        assert!(!under(".", "web"));
        // answers::in_scope agrees on every case it shares with under.
        for (p, s) in [
            ("web/app.py", "web"),
            ("webhooks/a.py", "web"),
            ("x/y.py", "."),
        ] {
            assert_eq!(under(p, s), in_scope(p, s), "{p} {s}");
        }
    }

    #[test]
    fn attribute_names_are_distinct() {
        let mut names: Vec<&str> = CODE_ATTRIBUTES.iter().map(|a| a.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), CODE_ATTRIBUTES.len());
    }
}
