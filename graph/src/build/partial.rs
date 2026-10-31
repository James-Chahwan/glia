//! GA.3: the per-file stitching seam, Python first.
//!
//! THE MODEL (GA.1's, `dev-notes/surface-replay-0.5.6.md`). A file's PARTIAL
//! is its SURFACE (its declarations: node ids, kinds, qnames, the nav tree,
//! field and return types, method signatures, properties, module-level
//! locals, its imports and heritage refs, the surface NavFacts; the engine's
//! `cache::surface_class`) plus its BODY stubs (call sites, non-heritage
//! refs, fn-scope locals, the body NavFacts, the node cells). A group's
//! SURFACE INDEX is [`RepoGraph::symbols`] plus the nav's declaration maps.
//! STITCHING resolves body stubs against that index: [`super::python_stitch`]
//! draws a file's IMPORTS edges, resolves its calls and refs and pairs its
//! methods with the interface members they implement.
//!
//! A BODY-ONLY edit leaves every edited file's surface as it was. The engine
//! checks that first (`engine::cache::surface_fingerprint` equal before and
//! after); the graph crate cannot see the engine, so [`rebuild_bodies`]
//! re-checks what the graph holds before it writes anything: the file's node
//! ids, kinds, names and qnames, its nav parents and children (in order),
//! field types, return types and method signatures, its MODULE scope's locals,
//! its properties and the import bindings its statements decide. Any
//! difference is [`Refused::SurfaceMoved`] and `g` is left as it was.
//!
//! FILE OWNERSHIP. A node belongs to the file whose MODULE is its nav
//! ancestor ([`file_nodes`]): a Python node's qname extends its file's MODULE
//! qname, so the nav tree names one file per node. A node no MODULE holds (an
//! ENDPOINT a client call mints, `endpoint:GET /x`) is one id every file
//! calling it shares: merging stacks each file's cells on it in parse order,
//! so its cells naming this file are one run, which is replaced in place
//! (every cell of such a node must name a file, else
//! [`Refused::Unsupported`]). An owned node carrying a second POSITION (two
//! parses declared one id) is refused as a shared node, never guessed.
//!
//! RE-STITCHING one file: (a) drop every edge whose `from` the file owns and
//! every unresolved call / ref it raised ([`drop_file_outputs`]); (b) the
//! owned nodes take the fresh parse's cells (the ids are equal, by the surface
//! check), the shared nodes their run; (c) the parse's own edges are pushed
//! again; (d) the owned scopes' `local_types` and `nav_facts` are replaced
//! ([`replace_file_nav`]); (e) [`super::python_stitch`] over the files' import
//! edges, calls and refs, method-level IMPLEMENTS for their types only; then
//! [`RepoGraph::canonicalize`] (GA.2), so the bytes are a clean build's.
//!
//! WHY NO OTHER FILE'S OUTPUT MOVES (verified against the Python builder):
//! - imports: a statement's decision reads the module and symbol tables,
//!   the star sources, the same-stem table and the namespace index, all
//!   surface (`imports::decide_imports_dotted`), never a binding or another
//!   file's body;
//! - calls (`calls::decide_call`): a site's receiver typing reads the
//!   CALLER's own `local_types` (its scope's, in its own file) and the
//!   `field_types` / `return_types` of the types it names (surface); the
//!   symbol tables and the repo-unique name index are surface; another
//!   file's node cells are read only for their file path (POSITION's `file`,
//!   which a body edit keeps). No lookup reads an edge or an unresolved list;
//! - refs (`calls::decide_ref`): the same tables, and the ref's own file for
//!   its evidence;
//! - method-level IMPLEMENTS: a pair hangs off a class-level IMPLEMENTS edge
//!   from one of the file's types, and a type's methods are its own children;
//! - the TypeScript-only passes `resolve_calls` / `resolve_refs` run on every
//!   call (member reads, page-navigation links) bind nothing in a Python
//!   group; a page-navigation ref in an edit is refused all the same.
//!
//! So a body-only edit leaves every other file's stitching valid: re-stitching
//! the edited files alone gives a clean build's graph.
//!
//! GA.7, GO ([`rebuild_go`]). The same seam, three differences. Ownership:
//! a split-file method (LA.23d) sits under its receiver STRUCT in another
//! file, so [`Owners`] names its declaring file and the merged parse is
//! re-parented the way the build re-parented it before it is compared. The
//! surface index is rebuilt from the graph and the group's parses (the
//! package model needs every file's import statements and embed refs, which
//! no graph keeps), and the edited files' import bindings are re-derived and
//! compared. And three Go passes read resolved calls or another file's body
//! facts, so a file holding their input is refused ([`go_refusal`]): a mount
//! fact, a ROUTE or HANDLED_BY ref, a JSON wire cell. The implicit and
//! method-level IMPLEMENTS and the split DEFINES are surface outputs with no
//! row: kept, never re-derived.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::ops::Range;

use glia_code_domain::wire::{ServerWire, WireForm};
use glia_code_domain::{
    CallSite, CodeNav, FileParse, ImportStmt, NavFact, UnresolvedRef, cell_type, edge_category,
    evidence, node_kind,
};
use glia_core::{Cell, Edge, EdgeCategoryId, Node, NodeId};

use super::{
    GoBody, PyBody, go_index, go_split_of, go_stitch, is_go_embed, merge_parses_with, python_stitch,
};
use crate::calls::EvidenceTally;
use crate::imports::{ImportEdge, RebindRefusal, python_rebind, resolve_imports_go};
use crate::types::RepoGraph;

/// GA.3: one file whose body changed, as its fresh parse.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BodyEdit {
    /// The edited file's parse, made exactly as the build made the old one
    /// (same repo, path and MODULE qname).
    pub parse: FileParse,
}

impl BodyEdit {
    /// An edit of the file `parse` was read from.
    pub fn new(parse: FileParse) -> Self {
        Self { parse }
    }
}

/// GA.3: why a body re-stitch was refused. The graph is untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refused {
    /// The file's declaration surface moved (a declaration, the nav tree, a
    /// type, a property, a module-level local or an import binding): every
    /// importer's stitching may move with it, so the group rebuilds.
    SurfaceMoved(String),
    /// The edit holds something this seam does not re-derive.
    Unsupported(&'static str),
    /// The edit's MODULE is not one of the graph's.
    UnknownFile(String),
}

impl Refused {
    /// The `refused=` field of the `[rebody]` line: one token.
    fn token(&self) -> String {
        match self {
            Refused::SurfaceMoved(path) => format!("surface_moved:{path}"),
            Refused::Unsupported(what) => format!("unsupported:{}", what.replace(' ', "_")),
            Refused::UnknownFile(path) => format!("unknown_file:{path}"),
        }
    }
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refused::SurfaceMoved(path) => write!(f, "declaration surface moved in {path}"),
            Refused::Unsupported(what) => write!(f, "not re-stitched: {what}"),
            Refused::UnknownFile(path) => write!(f, "no MODULE of the graph for {path}"),
        }
    }
}

impl std::error::Error for Refused {}

/// GA.3: what one body re-stitch did.
#[derive(Debug, Default, Clone, PartialEq)]
#[non_exhaustive]
pub struct RebodyStats {
    /// Files re-stitched.
    pub files: usize,
    /// Edges the files' old outputs held (dropped).
    pub edges_removed: usize,
    /// Edges drawn again: the parses' own and the stitch's.
    pub edges_added: usize,
    /// Call sites the stitch bound.
    pub calls_bound: usize,
    /// Refs the stitch bound.
    pub refs_bound: usize,
    /// Time the stitch spent building its repo-unique name indexes (only a
    /// site that falls back to one builds them).
    pub names_ms: f64,
}

/// What [`drop_file_outputs`] removed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileOutputs {
    pub(crate) edges: usize,
    pub(crate) unresolved_calls: usize,
    pub(crate) unresolved_refs: usize,
}

/// GA.7: node -> the file MODULE that declared it, for a node whose nav
/// ancestor is another file's (a Go split-file method, LA.23d, sits under its
/// receiver STRUCT); empty for a language whose nav tree names one file per
/// node (Python).
type Owners = HashMap<NodeId, NodeId>;

/// The MODULE `id` sits under (itself for a MODULE), walking `parent_of`
/// and stopping at a node `owners` names a file for; `None` when the chain
/// reaches no MODULE. Bounded by the nav's size, so a malformed cycle ends.
fn module_of(nav: &CodeNav, owners: &Owners, mut id: NodeId) -> Option<NodeId> {
    for _ in 0..=nav.parent_of.len() {
        if let Some(&module) = owners.get(&id) {
            return Some(module);
        }
        if nav.kind_by_id.get(&id) == Some(&node_kind::MODULE) {
            return Some(id);
        }
        id = *nav.parent_of.get(&id)?;
    }
    None
}

/// GA.3: the nodes of `g` the file of MODULE `module` owns: the MODULE and
/// every node whose nav ancestor it is (GA.7: or that `owners` places in it).
pub(crate) fn file_nodes(g: &RepoGraph, owners: &Owners, module: NodeId) -> HashSet<NodeId> {
    g.nodes
        .iter()
        .map(|n| n.id)
        .filter(|id| module_of(&g.nav, owners, *id) == Some(module))
        .collect()
}

/// GA.3: remove the stitch outputs of the files owning `owned`: every edge
/// whose `from` they own (GA.7: but the ones `keep` holds, a surface pass's
/// output the stitch does not re-derive), every unresolved call they raised
/// and every unresolved ref from them or with one of their `modules` as its
/// module.
pub(crate) fn drop_file_outputs(
    g: &mut RepoGraph,
    owned: &HashSet<NodeId>,
    modules: &HashSet<NodeId>,
    keep: impl Fn(&Edge) -> bool,
) -> FileOutputs {
    let (edges, calls, refs) = (
        g.edges.len(),
        g.unresolved_calls.len(),
        g.unresolved_refs.len(),
    );
    g.edges.retain(|e| !owned.contains(&e.from) || keep(e));
    g.unresolved_calls.retain(|c| !owned.contains(&c.from));
    g.unresolved_refs
        .retain(|r| !owned.contains(&r.from) && !modules.contains(&r.from_module));
    FileOutputs {
        edges: edges - g.edges.len(),
        unresolved_calls: calls - g.unresolved_calls.len(),
        unresolved_refs: refs - g.unresolved_refs.len(),
    }
}

/// GA.3: the body maps of the scopes in `old_ids` (`local_types`,
/// `nav_facts`) replaced by `new_nav`'s, the re-parsed file's. Every other
/// nav map is surface, equal by the surface check.
pub(crate) fn replace_file_nav(g: &mut RepoGraph, old_ids: &HashSet<NodeId>, new_nav: &CodeNav) {
    g.nav
        .local_types
        .retain(|scope, _| !old_ids.contains(scope));
    g.nav.nav_facts.retain(|scope, _| !old_ids.contains(scope));
    for (scope, locals) in &new_nav.local_types {
        g.nav.local_types.insert(*scope, locals.clone());
    }
    for (scope, facts) in &new_nav.nav_facts {
        g.nav.nav_facts.insert(*scope, facts.clone());
    }
}

/// One edit, checked against `g` and ready to splice.
struct Prepared {
    module: NodeId,
    qname: String,
    path: String,
    owned: HashSet<NodeId>,
    /// The parse merged alone ([`merge_parses_with`]: role overlays folded,
    /// as the group's merge folds them).
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    nav: CodeNav,
    imports: Vec<ImportStmt>,
    calls: Vec<CallSite>,
    refs: Vec<UnresolvedRef>,
    /// Shared nodes: (index in `g.nodes`, this file's run of its cells).
    runs: Vec<(usize, Range<usize>)>,
    /// The file's block of `g.nodes` (the nodes it minted first, from its
    /// MODULE to the next file's) and the block's indexes in the order the
    /// fresh parse mints them, the order a clean merge writes.
    block: Range<usize>,
    order: Vec<usize>,
}

/// What every edit is checked against, read once per call.
struct Ctx {
    /// The nodes declared in another file than their nav ancestor's
    /// ([`Owners`]).
    owners: Owners,
    /// Node id -> its index in `g.nodes`.
    index: HashMap<NodeId, usize>,
    /// The indexes of `g.nodes`' MODULEs, ascending: a clean merge writes
    /// each parse's new nodes as one block, its MODULE first.
    modules: Vec<usize>,
    /// File -> the nodes no MODULE holds with a cell naming that file, in
    /// `g.nodes` order.
    shared_by_file: HashMap<String, Vec<NodeId>>,
}

impl Ctx {
    fn read(g: &RepoGraph, owners: Owners) -> Ctx {
        let mut index: HashMap<NodeId, usize> = HashMap::with_capacity(g.nodes.len());
        let mut modules = Vec::new();
        let mut shared_by_file: HashMap<String, Vec<NodeId>> = HashMap::new();
        for (at, n) in g.nodes.iter().enumerate() {
            index.entry(n.id).or_insert(at);
            if g.nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE) {
                modules.push(at);
                continue;
            }
            if g.nav.parent_of.contains_key(&n.id) {
                continue;
            }
            for c in &n.cells {
                if let Some(file) = cell_file(c) {
                    let ids = shared_by_file.entry(file).or_default();
                    if !ids.contains(&n.id) {
                        ids.push(n.id);
                    }
                }
            }
        }
        Ctx {
            owners,
            index,
            modules,
            shared_by_file,
        }
    }
}

/// The file a cell names: a POSITION's, an ENDPOINT_HIT's or a JSON
/// ROUTE_METHOD's `file`.
fn cell_file(cell: &Cell) -> Option<String> {
    evidence::locate(std::slice::from_ref(cell)).map(|(file, _)| file)
}

/// The run of `cells` naming `path`, when every cell names a file and the
/// ones naming `path` are one non-empty run.
fn cell_run(cells: &[Cell], path: &str) -> Option<Range<usize>> {
    let mut hits: Vec<usize> = Vec::new();
    for (at, c) in cells.iter().enumerate() {
        if cell_file(c)? == path {
            hits.push(at);
        }
    }
    let (first, last) = (*hits.first()?, *hits.last()?);
    (last - first + 1 == hits.len()).then_some(first..last + 1)
}

/// Check one edit against `g` (read-only) and merge its parse.
fn prepare(g: &RepoGraph, ctx: &Ctx, edit: BodyEdit) -> Result<Prepared, Refused> {
    let index = &ctx.index;
    let (mut mini, imports, calls, refs) = merge_parses_with(g.repo, vec![edit.parse], false);
    let mut modules = mini
        .nodes
        .iter()
        .filter(|n| mini.nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE));
    let (Some(module), None) = (modules.next(), modules.next()) else {
        return Err(Refused::Unsupported(
            "a parse holding other than one MODULE",
        ));
    };
    let module_id = module.id;
    let Some((path, _)) = evidence::locate(&module.cells) else {
        return Err(Refused::Unsupported("a MODULE without a file"));
    };
    let qname = mini
        .nav
        .qname_by_id
        .get(&module_id)
        .cloned()
        .unwrap_or_default();
    if g.nav.kind_by_id.get(&module_id) != Some(&node_kind::MODULE)
        || !index.contains_key(&module_id)
    {
        return Err(Refused::UnknownFile(path));
    }
    let moved = || Refused::SurfaceMoved(path.clone());
    // GA.7: a method this file declares under another file's type sits there
    // in the merged parse too, as the build re-parented it.
    place_owned(g, &ctx.owners, &mut mini, module_id);

    // The declarations: one node set, each node named and placed alike.
    let owned = file_nodes(g, &ctx.owners, module_id);
    let new_owned = file_nodes(&mini, &ctx.owners, module_id);
    if owned != new_owned {
        // A node both parses hold, placed in another file by the merge (two
        // files minting one id under their own MODULE: the last one wins).
        let shared = owned
            .symmetric_difference(&new_owned)
            .any(|id| index.contains_key(id) && mini.nav.kind_by_id.contains_key(id));
        return Err(if shared {
            Refused::Unsupported("shared node")
        } else {
            moved()
        });
    }
    // A node's children this file declares, in order (a STRUCT's methods
    // other files declare are theirs).
    let own_children = |nav: &CodeNav, id: &NodeId| -> Vec<NodeId> {
        nav.children_of
            .get(id)
            .map(|kids| kids.iter().filter(|k| owned.contains(k)).copied().collect())
            .unwrap_or_default()
    };
    let (a, b) = (&g.nav, &mini.nav);
    for n in &mini.nodes {
        let id = n.id;
        let named = a.name_by_id.get(&id) == b.name_by_id.get(&id)
            && a.qname_by_id.get(&id) == b.qname_by_id.get(&id)
            && a.kind_by_id.get(&id) == b.kind_by_id.get(&id);
        if !named || !index.contains_key(&id) {
            return Err(moved());
        }
        if !owned.contains(&id) {
            continue;
        }
        let placed = a.parent_of.get(&id) == b.parent_of.get(&id)
            && own_children(a, &id) == own_children(b, &id)
            && a.field_types.get(&id) == b.field_types.get(&id)
            && a.return_types.get(&id) == b.return_types.get(&id);
        if !placed {
            return Err(moved());
        }
        if a.method_sigs.get(&id) != b.method_sigs.get(&id) {
            // CI.6 rewrote the built signature's in-repo type aliases.
            let aliased = a
                .nav_facts
                .values()
                .flatten()
                .any(|f| matches!(f, NavFact::TypeAlias { .. }));
            return Err(if aliased {
                Refused::Unsupported("type alias")
            } else {
                moved()
            });
        }
    }
    if a.local_types.get(&module_id) != b.local_types.get(&module_id) {
        return Err(moved());
    }
    let old_props: HashSet<NodeId> = owned
        .iter()
        .filter(|id| g.properties.contains(id))
        .copied()
        .collect();
    let new_props = &mini.properties;
    if !new_props.is_subset(&owned) {
        return Err(Refused::Unsupported("a property outside its file"));
    }
    if &old_props != new_props {
        return Err(moved());
    }

    // An owned node with a second POSITION was declared by two parses.
    for n in g.nodes.iter().filter(|n| owned.contains(&n.id)) {
        let mut positions = n.cells.iter().filter(|c| c.kind == cell_type::POSITION);
        let first = positions.next();
        if positions.next().is_some()
            || first.is_some_and(|c| cell_file(c).as_deref() != Some(&path))
        {
            return Err(Refused::Unsupported("shared node"));
        }
    }
    // A node no MODULE holds: this file's run of its stacked cells.
    let mut runs = Vec::new();
    for n in mini.nodes.iter().filter(|n| !owned.contains(&n.id)) {
        let Some(&at) = index.get(&n.id) else {
            return Err(moved());
        };
        if g.nav.parent_of.contains_key(&n.id) || mini.nav.parent_of.contains_key(&n.id) {
            return Err(Refused::Unsupported("shared node"));
        }
        let fresh = !n.cells.is_empty()
            && n.cells
                .iter()
                .all(|c| cell_file(c).as_deref() == Some(path.as_str()));
        match cell_run(&g.nodes[at].cells, &path) {
            Some(run) if fresh => runs.push((at, run)),
            _ => return Err(Refused::Unsupported("shared node")),
        }
    }

    // Every node no MODULE holds that this file minted is still minted.
    let minted: HashSet<NodeId> = mini.nodes.iter().map(|n| n.id).collect();
    if ctx
        .shared_by_file
        .get(&path)
        .is_some_and(|ids| ids.iter().any(|id| !minted.contains(id)))
    {
        return Err(moved());
    }
    // The file's block, in the order the fresh parse mints it.
    let start = index.get(&module_id).copied().unwrap_or(usize::MAX);
    let end = ctx
        .modules
        .iter()
        .copied()
        .find(|&m| m > start)
        .unwrap_or(g.nodes.len());
    let order: Vec<usize> = mini
        .nodes
        .iter()
        .filter_map(|n| index.get(&n.id).copied())
        .filter(|&at| at >= start)
        .collect();
    if order.len() != end.saturating_sub(start) || order.iter().any(|&at| at >= end) {
        return Err(moved());
    }

    // The body stubs all hang off the file's own nodes.
    if mini.edges.iter().any(|e| !owned.contains(&e.from)) {
        return Err(Refused::Unsupported("a parse edge outside its file"));
    }
    if calls.iter().any(|c| !owned.contains(&c.from)) {
        return Err(Refused::Unsupported("a call site outside its file"));
    }
    if refs
        .iter()
        .any(|r| r.category == edge_category::NAVIGATES_TO)
    {
        return Err(Refused::Unsupported("page navigation"));
    }
    if refs
        .iter()
        .any(|r| !owned.contains(&r.from) || r.from_module != module_id)
    {
        return Err(Refused::Unsupported("a ref outside its file"));
    }
    if imports.iter().any(|i| i.from_module != qname) {
        return Err(Refused::Unsupported("an import outside its file"));
    }
    if b.local_types.keys().any(|s| !owned.contains(s))
        || b.nav_facts.keys().any(|s| !owned.contains(s))
    {
        return Err(Refused::Unsupported("a nav fact outside its file"));
    }

    Ok(Prepared {
        module: module_id,
        qname,
        path,
        owned,
        nodes: mini.nodes,
        edges: mini.edges,
        nav: mini.nav,
        imports,
        calls,
        refs,
        runs,
        block: start..end,
        order,
    })
}

/// GA.3: [`super::rebuild_python_bodies`] without its marker. Every check
/// runs before the first write, so a refusal leaves `g` as it was.
pub(crate) fn rebuild_bodies(
    g: &mut RepoGraph,
    edits: Vec<BodyEdit>,
) -> Result<RebodyStats, Refused> {
    if !g.symbols.home_module.is_empty() {
        return Err(Refused::Unsupported("shared package"));
    }
    let ctx = Ctx::read(g, Owners::new());
    let index = &ctx.index;
    let mut prepared = edits
        .into_iter()
        .map(|e| prepare(g, &ctx, e))
        .collect::<Result<Vec<Prepared>, Refused>>()?;
    prepared.sort_by(|a, b| a.qname.cmp(&b.qname).then(a.path.cmp(&b.path)));
    if prepared.windows(2).any(|w| w[0].module == w[1].module) {
        return Err(Refused::Unsupported("a file edited twice"));
    }
    // A shared node's cells move by run; an edge of its own would not.
    let shared: HashSet<NodeId> = prepared
        .iter()
        .flat_map(|p| p.runs.iter().map(|(at, _)| g.nodes[*at].id))
        .collect();
    if !shared.is_empty() && g.edges.iter().any(|e| shared.contains(&e.from)) {
        return Err(Refused::Unsupported("an edge from a shared node"));
    }
    let files: Vec<(NodeId, &[ImportStmt])> = prepared
        .iter()
        .map(|p| (p.module, p.imports.as_slice()))
        .collect();
    let import_edges: Vec<Vec<ImportEdge>> = python_rebind(g, &files).map_err(|r| match r {
        RebindRefusal::BindingsMoved(at) => Refused::SurfaceMoved(prepared[at].path.clone()),
        RebindRefusal::StarRowTie => Refused::Unsupported("star imports sharing a row"),
    })?;

    // Writes from here on.
    let owned: HashSet<NodeId> = prepared
        .iter()
        .flat_map(|p| p.owned.iter().copied())
        .collect();
    let modules: HashSet<NodeId> = prepared.iter().map(|p| p.module).collect();
    let dropped = drop_file_outputs(g, &owned, &modules, |_| false);
    let kept = g.edges.len();
    let mut body = PyBody::default();
    let mut types: HashSet<NodeId> = HashSet::new();
    for p in prepared {
        let Prepared {
            owned,
            nodes,
            edges,
            nav,
            calls,
            refs,
            runs,
            block,
            order,
            ..
        } = p;
        let mut run_of: HashMap<usize, Range<usize>> = runs.into_iter().collect();
        for n in nodes {
            let Some(&at) = index.get(&n.id) else {
                continue;
            };
            match run_of.remove(&at) {
                Some(run) => {
                    g.nodes[at].cells.splice(run, n.cells);
                }
                None => g.nodes[at].cells = n.cells,
            }
        }
        reorder_block(g, block, &order);
        g.edges.extend(edges);
        replace_file_nav(g, &owned, &nav);
        body.calls.extend(calls);
        body.refs.extend(refs);
        types.extend(owned);
    }
    let imports: Vec<ImportEdge> = import_edges.into_iter().flatten().collect();
    let mut tally = EvidenceTally::default();
    python_stitch(g, &imports, &body, Some(&types), &mut tally);
    let added = g.edges.len() - kept;
    g.canonicalize();
    Ok(RebodyStats {
        files: modules.len(),
        edges_removed: dropped.edges,
        edges_added: added,
        calls_bound: tally.calls_bound(),
        refs_bound: tally.refs_bound(),
        names_ms: tally.names_ms(),
    })
}

/// GA.7: re-parent, in the edited file's merged parse `mini`, every method
/// `owners` places in file `module` whose nav parent the build moved to a
/// STRUCT of another file (LA.23d's split receivers), as the build did:
/// under that parent, out of the MODULE's children.
fn place_owned(g: &RepoGraph, owners: &Owners, mini: &mut RepoGraph, module: NodeId) {
    if owners.is_empty() {
        return;
    }
    let moved: Vec<(NodeId, NodeId)> = mini
        .nodes
        .iter()
        .filter(|n| owners.get(&n.id) == Some(&module))
        .filter(|n| mini.nav.parent_of.get(&n.id) == Some(&module))
        .filter_map(|n| {
            let parent = *g.nav.parent_of.get(&n.id)?;
            (parent != module).then_some((n.id, parent))
        })
        .collect();
    for (id, parent) in moved {
        mini.nav.parent_of.insert(id, parent);
        if let Some(kids) = mini.nav.children_of.get_mut(&module) {
            kids.retain(|k| *k != id);
        }
    }
}

/// GA.7: why a Go edit is not re-stitched, beyond GA.3's checks: its old
/// (the file's nodes in `g`) or new (`parse`) parse holds what another file's resolution reads
/// through resolved calls. `mount`: a CB.20 mount fact (a `MountArg` flows
/// through this file's CALLS into another file's ROUTEs, a `FieldMount` keys
/// the ROUTEs of the field's readers); `route`: a ROUTE or a HANDLED_BY ref
/// or edge (CI.4 binds handlers through the package, CB.20 re-keys the
/// ROUTEs); `wire`: a json_response / json_bind wire cell (DT.1f types it
/// through other files' facts and draws its USES).
fn go_refusal(g: &RepoGraph, ctx: &Ctx, parse: &FileParse) -> Option<&'static str> {
    let nav = &parse.nav;
    let mut modules = parse
        .nodes
        .iter()
        .filter(|n| nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE));
    let module = modules.next()?;
    let path = evidence::locate(&module.cells).map(|(file, _)| file);
    let owned = if ctx.index.contains_key(&module.id) {
        file_nodes(g, &ctx.owners, module.id)
    } else {
        HashSet::new()
    };
    let mount = |f: &NavFact| matches!(f, NavFact::MountArg { .. } | NavFact::FieldMount { .. });
    let old_facts = owned
        .iter()
        .filter_map(|id| g.nav.nav_facts.get(id))
        .flatten();
    if old_facts.chain(nav.nav_facts.values().flatten()).any(mount) {
        return Some("mount");
    }
    let shared = path
        .as_ref()
        .and_then(|p| ctx.shared_by_file.get(p))
        .into_iter()
        .flatten();
    let is_route = |nav: &CodeNav, id: &NodeId| nav.kind_by_id.get(id) == Some(&node_kind::ROUTE);
    let handled = |category: EdgeCategoryId| category == edge_category::HANDLED_BY;
    let route = owned.iter().chain(shared).any(|id| is_route(&g.nav, id))
        || parse.nodes.iter().any(|n| is_route(nav, &n.id))
        || parse.refs.iter().any(|r| handled(r.category))
        || g.unresolved_refs.iter().any(|r| {
            handled(r.category) && (owned.contains(&r.from) || r.from_module == module.id)
        })
        || g.edges
            .iter()
            .any(|e| handled(e.category) && owned.contains(&e.from));
    if route {
        return Some("route");
    }
    let wire = |c: &Cell| {
        ServerWire::from_cell(c)
            .is_some_and(|w| matches!(w.form, WireForm::JsonResponse | WireForm::JsonBind))
    };
    let old_wire = g
        .nodes
        .iter()
        .filter(|n| owned.contains(&n.id))
        .any(|n| n.cells.iter().any(wire));
    if old_wire || parse.nodes.iter().any(|n| n.cells.iter().any(wire)) {
        return Some("wire");
    }
    None
}

/// GA.7: [`super::rebuild_go_bodies`] without its marker: GA.3's checks
/// ([`prepare`], file ownership following the split receivers), the Go
/// refusals ([`go_refusal`]), the package model rebuilt from `group`'s
/// statements and embed refs ([`super::go_index`]) and each edited file's
/// import bindings re-derived and compared, all before the first write; then
/// the files' outputs dropped (the implicit / method-level IMPLEMENTS and the
/// split DEFINES kept: surface passes' outputs, carrying no row), their
/// cells, parse edges and body nav replaced, their IMPORTS edges drawn again
/// and [`super::go_stitch`] over their calls and refs; then
/// [`RepoGraph::canonicalize`].
///
/// WHY NO OTHER FILE'S OUTPUT MOVES (verified against the Go builder,
/// beside the Python reading in the module doc): the Go call hook types a
/// receiver from the CALLER's own locals and its file's imports, and from
/// the surface (package vars are MODULE-scope locals, field / return types,
/// signatures, the package model, the promoted members), never an edge or
/// another file's body; the value refs bind through the same tables; the
/// passes that DO read resolved calls or another file's body facts are the
/// three the refusals keep out: CB.20's mounts (a `MountArg`'s caller CALLS),
/// CI.4's type handlers and DT.1f's wire typing.
pub(crate) fn rebuild_go(
    g: &mut RepoGraph,
    edits: Vec<BodyEdit>,
    group: &[FileParse],
) -> Result<RebodyStats, Refused> {
    if !g.symbols.home_module.is_empty() {
        return Err(Refused::Unsupported("shared package"));
    }
    let split = go_split_of(g);
    let owners: Owners = split
        .bound
        .iter()
        .map(|&(method, module, _)| (method, module))
        .collect();
    let ctx = Ctx::read(g, owners);
    let index = &ctx.index;
    // The Go refusals first, so a refused file reads as why, whatever else
    // moved; the first reason in this order over every edit.
    let reasons: Vec<&'static str> = edits
        .iter()
        .filter_map(|e| go_refusal(g, &ctx, &e.parse))
        .collect();
    if let Some(why) = ["mount", "route", "wire"]
        .into_iter()
        .find(|r| reasons.contains(r))
    {
        return Err(Refused::Unsupported(why));
    }
    let mut prepared = edits
        .into_iter()
        .map(|e| prepare(g, &ctx, e))
        .collect::<Result<Vec<Prepared>, Refused>>()?;
    prepared.sort_by(|a, b| a.qname.cmp(&b.qname).then(a.path.cmp(&b.path)));
    if prepared.windows(2).any(|w| w[0].module == w[1].module) {
        return Err(Refused::Unsupported("a file edited twice"));
    }
    let shared: HashSet<NodeId> = prepared
        .iter()
        .flat_map(|p| p.runs.iter().map(|(at, _)| g.nodes[*at].id))
        .collect();
    if !shared.is_empty() && g.edges.iter().any(|e| shared.contains(&e.from)) {
        return Err(Refused::Unsupported("an edge from a shared node"));
    }

    // The surface index, from every statement and embed ref of the group.
    let imports: Vec<ImportStmt> = group.iter().flat_map(|p| p.imports.clone()).collect();
    let embed_refs: Vec<UnresolvedRef> = group
        .iter()
        .flat_map(|p| p.refs.iter())
        .filter(|r| is_go_embed(&g.nav, r))
        .cloned()
        .collect();
    let mut surface = go_index(g, split, &imports, &embed_refs);
    // Each file's IMPORTS edges, drawn and taken back: its bindings must be
    // the graph's, else the surface moved (`g` restored either way).
    let mut import_edges: Vec<Edge> = Vec::new();
    for p in &prepared {
        let old = g.symbols.module_import_bindings.remove(&p.module);
        let mark = g.edges.len();
        resolve_imports_go(g, &p.imports, &surface.packages);
        let drawn = g.edges.split_off(mark);
        let fresh = g.symbols.module_import_bindings.remove(&p.module);
        if let Some(old) = &old {
            g.symbols
                .module_import_bindings
                .insert(p.module, old.clone());
        }
        if fresh != old {
            return Err(Refused::SurfaceMoved(p.path.clone()));
        }
        import_edges.extend(drawn);
    }

    // Writes from here on.
    let owned: HashSet<NodeId> = prepared
        .iter()
        .flat_map(|p| p.owned.iter().copied())
        .collect();
    let modules: HashSet<NodeId> = prepared.iter().map(|p| p.module).collect();
    let dropped = drop_file_outputs(g, &owned, &modules, is_go_surface_edge);
    let kept = g.edges.len();
    let mut body = GoBody {
        calls: Vec::new(),
        refs: Vec::new(),
    };
    for p in prepared {
        let Prepared {
            owned,
            nodes,
            edges,
            nav,
            calls,
            refs,
            runs,
            block,
            order,
            ..
        } = p;
        let mut run_of: HashMap<usize, Range<usize>> = runs.into_iter().collect();
        for n in nodes {
            let Some(&at) = index.get(&n.id) else {
                continue;
            };
            match run_of.remove(&at) {
                Some(run) => {
                    g.nodes[at].cells.splice(run, n.cells);
                }
                None => g.nodes[at].cells = n.cells,
            }
        }
        reorder_block(g, block, &order);
        g.edges.extend(edges);
        replace_file_nav(g, &owned, &nav);
        body.calls.extend(calls);
        body.refs
            .extend(refs.into_iter().filter(|r| !is_go_embed(&g.nav, r)));
    }
    g.edges.extend(import_edges);
    let mut tally = EvidenceTally::default();
    go_stitch(g, &mut surface, body, Some(&owned), &mut tally);
    let added = g.edges.len() - kept;
    g.canonicalize();
    Ok(RebodyStats {
        files: modules.len(),
        edges_removed: dropped.edges,
        edges_added: added,
        calls_bound: tally.calls_bound(),
        refs_bound: tally.refs_bound(),
        names_ms: tally.names_ms(),
    })
}

/// GA.7: an edge a Go re-stitch keeps although its `from` is an edited
/// file's: an IMPLEMENTS the implicit or method-level pass derived from
/// method sets and signatures (`graph:iface`), or a split receiver's
/// struct -> method DEFINES (`split_receiver`). Both are surface passes'
/// outputs and carry no row, so a body-only edit leaves them as they are.
fn is_go_surface_edge(e: &Edge) -> bool {
    evidence::Evidence::of(e).is_some_and(|ev| match ev.emitter.as_str() {
        "graph:iface" => true,
        "graph:go_packages" => ev.rule.as_deref() == Some("split_receiver"),
        _ => false,
    })
}

/// Put `g.nodes[block]` in `order` (the block's own indexes, each once): a
/// body edit that mints a shared node at another statement moves it within
/// its file's block, as a clean merge would.
fn reorder_block(g: &mut RepoGraph, block: Range<usize>, order: &[usize]) {
    if order.iter().copied().eq(block.clone()) {
        return;
    }
    let start = block.start;
    let mut slots: Vec<Option<Node>> = g
        .nodes
        .splice(block, std::iter::empty())
        .map(Some)
        .collect();
    let placed: Vec<Node> = order
        .iter()
        .filter_map(|&at| slots.get_mut(at - start).and_then(Option::take))
        .collect();
    g.nodes.splice(start..start, placed);
}

/// The `[rebody]` fired_on line of one call.
pub(crate) fn marker(lang: &str, result: &Result<RebodyStats, Refused>) -> String {
    let (s, refused) = match result {
        Ok(s) => (s.clone(), "none".to_string()),
        Err(r) => (RebodyStats::default(), r.token()),
    };
    format!(
        "[rebody] lang={lang} files={} edges -{} +{} calls_bound={} refs_bound={} names_ms={:.1} \
         refused={refused}",
        s.files, s.edges_removed, s.edges_added, s.calls_bound, s.refs_bound, s.names_ms
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use glia_code_domain::NavFact;
    use glia_core::RepoId;

    use super::*;
    use crate::build::{build_go, build_python, rebuild_go_bodies, rebuild_python_bodies};

    fn repo() -> RepoId {
        RepoId::from_canonical("test://rebody")
    }

    fn parse(path: &str, src: &str) -> FileParse {
        let qname = path.trim_end_matches(".py").replace('/', "::");
        glia_parser_python::parse_file(src, path, &qname, repo()).expect("parse")
    }

    const MODELS: &str = "\
import requests


class Repo:
    def save(self, x):
        return x

    def load(self):
        return self.save(1)


def helper():
    return 1


def fetch():
    return requests.get(\"/api/items\")
";

    const SERVICE: &str = "\
import requests
from app.models import Repo, helper
from app import fetch
import app.util


class Service:
    def __init__(self):
        self.repo = Repo()

    def run(self):
        helper()
        self.repo.save(2)
        return app.util.fmt(\"x\")


def main():
    s = Service()
    s.run()
    requests.get(\"/api/items\")
";

    const UTIL: &str = "\
def fmt(x):
    return x


def unused():
    return fmt(1)
";

    const INIT: &str = "from .models import *\n";

    /// The tree before any edit, in parse order.
    fn base() -> Vec<(&'static str, String)> {
        vec![
            ("app/__init__.py", INIT.to_string()),
            ("app/models.py", MODELS.to_string()),
            ("app/service.py", SERVICE.to_string()),
            ("app/util.py", UTIL.to_string()),
        ]
    }

    fn parses(tree: &[(&'static str, String)]) -> Vec<FileParse> {
        tree.iter().map(|(p, s)| parse(p, s)).collect()
    }

    fn clean(tree: &[(&'static str, String)]) -> RepoGraph {
        let mut g = build_python(repo(), parses(tree)).expect("build");
        g.canonicalize();
        g
    }

    /// `tree` with `path` replaced by `src`.
    fn edited(
        tree: &[(&'static str, String)],
        path: &str,
        src: &str,
    ) -> Vec<(&'static str, String)> {
        tree.iter()
            .map(|(p, s)| {
                let s = if *p == path {
                    src.to_string()
                } else {
                    s.clone()
                };
                (*p, s)
            })
            .collect()
    }

    fn sorted<V: Clone>(m: &HashMap<NodeId, V>) -> Vec<(u64, V)> {
        let mut v: Vec<(u64, V)> = m.iter().map(|(k, v)| (k.0, v.clone())).collect();
        v.sort_by_key(|(k, _)| *k);
        v
    }

    fn sorted_inner<V: Clone + Ord>(
        m: &HashMap<NodeId, HashMap<String, V>>,
    ) -> Vec<(u64, BTreeMap<String, V>)> {
        let mut v: Vec<(u64, BTreeMap<String, V>)> = m
            .iter()
            .map(|(k, v)| (k.0, v.iter().map(|(a, b)| (a.clone(), b.clone())).collect()))
            .collect();
        v.sort_by_key(|(k, _)| *k);
        v
    }

    fn ids(m: &HashMap<NodeId, HashMap<String, NodeId>>) -> Vec<(u64, BTreeMap<String, u64>)> {
        let mut v: Vec<(u64, BTreeMap<String, u64>)> = m
            .iter()
            .map(|(k, v)| (k.0, v.iter().map(|(a, b)| (a.clone(), b.0)).collect()))
            .collect();
        v.sort_by_key(|(k, _)| *k);
        v
    }

    /// Field by field: nodes, edges, unresolved lists, properties, the nav
    /// maps and the symbol tables (maps as sorted vecs).
    fn assert_same(got: &RepoGraph, want: &RepoGraph, label: &str) {
        assert_eq!(got.nodes, want.nodes, "{label}: nodes");
        assert_eq!(got.edges, want.edges, "{label}: edges");
        assert_eq!(
            got.unresolved_calls, want.unresolved_calls,
            "{label}: unresolved_calls"
        );
        assert_eq!(
            got.unresolved_refs, want.unresolved_refs,
            "{label}: unresolved_refs"
        );
        assert_eq!(got.properties, want.properties, "{label}: properties");
        let (a, b) = (&got.nav, &want.nav);
        assert_eq!(
            sorted(&a.name_by_id),
            sorted(&b.name_by_id),
            "{label}: names"
        );
        assert_eq!(
            sorted(&a.qname_by_id),
            sorted(&b.qname_by_id),
            "{label}: qnames"
        );
        let kinds = |n: &CodeNav| -> Vec<(u64, u32)> {
            sorted(&n.kind_by_id)
                .into_iter()
                .map(|(k, v)| (k, v.0))
                .collect()
        };
        assert_eq!(kinds(a), kinds(b), "{label}: kinds");
        let parents = |n: &CodeNav| -> Vec<(u64, u64)> {
            sorted(&n.parent_of)
                .into_iter()
                .map(|(k, v)| (k, v.0))
                .collect()
        };
        assert_eq!(parents(a), parents(b), "{label}: parents");
        let children = |n: &CodeNav| -> Vec<(u64, Vec<u64>)> {
            sorted(&n.children_of)
                .into_iter()
                .map(|(k, v)| (k, v.iter().map(|c| c.0).collect()))
                .collect()
        };
        assert_eq!(children(a), children(b), "{label}: children");
        assert_eq!(
            sorted_inner(&a.field_types),
            sorted_inner(&b.field_types),
            "{label}: field_types"
        );
        assert_eq!(
            sorted_inner(&a.local_types),
            sorted_inner(&b.local_types),
            "{label}: local_types"
        );
        let facts = |n: &CodeNav| -> Vec<(u64, Vec<NavFact>)> { sorted(&n.nav_facts) };
        assert_eq!(facts(a), facts(b), "{label}: nav_facts");
        assert_eq!(
            sorted(&a.return_types),
            sorted(&b.return_types),
            "{label}: returns"
        );
        assert_eq!(
            sorted(&a.method_sigs),
            sorted(&b.method_sigs),
            "{label}: sigs"
        );
        let (s, t) = (&got.symbols, &want.symbols);
        let modules = |m: &HashMap<String, NodeId>| -> BTreeMap<String, u64> {
            m.iter().map(|(k, v)| (k.clone(), v.0)).collect()
        };
        assert_eq!(modules(&s.module_by_qname), modules(&t.module_by_qname));
        assert_eq!(ids(&s.module_symbols), ids(&t.module_symbols), "{label}");
        assert_eq!(ids(&s.class_methods), ids(&t.class_methods), "{label}");
        assert_eq!(
            ids(&s.module_import_bindings),
            ids(&t.module_import_bindings),
            "{label}"
        );
    }

    /// Rebuild `path` as `src` on the base graph, compare with a clean build
    /// of the edited tree, and return the stats.
    fn rebody(path: &str, src: &str, label: &str) -> RebodyStats {
        let tree = base();
        let mut g = clean(&tree);
        let after = edited(&tree, path, src);
        let stats = rebuild_python_bodies(&mut g, vec![BodyEdit::new(parse(path, src))])
            .unwrap_or_else(|e| panic!("{label}: refused: {e}"));
        assert_same(&g, &clean(&after), label);
        stats
    }

    /// The fixture exercises what the seam must re-derive: a star re-export
    /// binding, a shared ENDPOINT, receiver-typed and module calls.
    #[test]
    fn base_fixture_reaches_every_path() {
        let g = clean(&base());
        let rules: Vec<String> = g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::IMPORTS)
            .filter_map(|e| evidence::Evidence::of(e).and_then(|ev| ev.rule))
            .collect();
        assert!(rules.iter().any(|r| r == "star_import"), "{rules:?}");
        assert!(
            rules
                .iter()
                .any(|r| r == "star_reexport" || r == "package_init"),
            "{rules:?}"
        );
        let endpoint = g
            .nodes
            .iter()
            .find(|n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
            .expect("an ENDPOINT");
        assert_eq!(endpoint.cells.len(), 2, "one hit per calling file");
        assert!(g.edges.iter().any(|e| e.category == edge_category::CALLS));
    }

    #[test]
    fn a_call_added() {
        let src = SERVICE.replace("    s.run()\n", "    s.run()\n    helper()\n");
        let s = rebody("app/service.py", &src, "call added");
        assert_eq!(s.files, 1);
        assert!(s.edges_added > 0 && s.edges_removed > 0, "{s:?}");
    }

    #[test]
    fn a_call_removed() {
        let src = SERVICE.replace("        helper()\n", "");
        rebody("app/service.py", &src, "call removed");
    }

    #[test]
    fn a_call_moved_down_three_lines() {
        let src = SERVICE.replace(
            "        helper()\n        self.repo.save(2)\n",
            "        a = 1\n        b = 2\n        c = a + b\n        helper()\n        self.repo.save(2)\n",
        );
        rebody("app/service.py", &src, "call moved");
    }

    #[test]
    fn a_body_line_above_two_defs() {
        let src = MODELS.replace(
            "    def save(self, x):\n        return x\n",
            "    def save(self, x):\n        y = x\n        return y\n",
        );
        rebody("app/models.py", &src, "line above two defs");
    }

    #[test]
    fn a_method_body_changed() {
        let src = MODELS.replace(
            "        return self.save(1)\n",
            "        out = self.save(2)\n        helper()\n        return requests.get(\"/api/items\")\n",
        );
        rebody("app/models.py", &src, "method body");
    }

    #[test]
    fn an_imported_file_edited() {
        let src = UTIL.replace("    return x\n", "    print(x)\n    return unused()\n");
        rebody("app/util.py", &src, "imported file");
    }

    /// Two files in one call, the shared ENDPOINT's runs moved in both.
    #[test]
    fn two_files_at_once() {
        let tree = base();
        let mut g = clean(&tree);
        let models = MODELS.replace("def fetch():\n", "def fetch():\n    helper()\n");
        let service = SERVICE.replace("    s.run()\n", "    s.run()\n    fetch()\n");
        let after = edited(
            &edited(&tree, "app/models.py", &models),
            "app/service.py",
            &service,
        );
        let stats = rebuild_python_bodies(
            &mut g,
            vec![
                BodyEdit::new(parse("app/service.py", &service)),
                BodyEdit::new(parse("app/models.py", &models)),
            ],
        )
        .expect("rebuild");
        assert_eq!(stats.files, 2);
        assert_same(&g, &clean(&after), "two files");
    }

    /// A def added moves the surface: refused, and the graph untouched.
    #[test]
    fn a_def_added_is_refused() {
        let tree = base();
        let mut g = clean(&tree);
        let before = g.clone();
        let src = format!("{UTIL}\n\ndef extra():\n    return 2\n");
        let got = rebuild_python_bodies(&mut g, vec![BodyEdit::new(parse("app/util.py", &src))]);
        assert_eq!(got, Err(Refused::SurfaceMoved("app/util.py".to_string())));
        assert_same(&g, &before, "refused");
        assert!(
            marker("python", &got).ends_with("refused=surface_moved:app/util.py"),
            "{}",
            marker("python", &got)
        );
    }

    /// A new in-repo import moves the file's bindings: refused.
    #[test]
    fn an_import_added_is_refused() {
        let tree = base();
        let mut g = clean(&tree);
        let src = format!("from app.models import helper\n{UTIL}");
        let got = rebuild_python_bodies(&mut g, vec![BodyEdit::new(parse("app/util.py", &src))]);
        assert_eq!(got, Err(Refused::SurfaceMoved("app/util.py".to_string())));
    }

    /// A file that stops minting a shared ENDPOINT moved its node set:
    /// refused, never left with the stale hit.
    #[test]
    fn a_dropped_shared_mint_is_refused() {
        let mut g = clean(&base());
        let src = SERVICE.replace("    requests.get(\"/api/items\")\n", "    pass\n");
        let got = rebuild_python_bodies(&mut g, vec![BodyEdit::new(parse("app/service.py", &src))]);
        assert_eq!(
            got,
            Err(Refused::SurfaceMoved("app/service.py".to_string()))
        );
    }

    #[test]
    fn an_unknown_file_is_refused() {
        let mut g = clean(&base());
        let got = rebuild_python_bodies(&mut g, vec![BodyEdit::new(parse("app/new.py", UTIL))]);
        assert_eq!(got, Err(Refused::UnknownFile("app/new.py".to_string())));
    }

    #[test]
    fn the_marker_reads_the_stats() {
        let s = RebodyStats {
            files: 2,
            edges_removed: 7,
            edges_added: 9,
            calls_bound: 3,
            refs_bound: 1,
            names_ms: 0.3,
        };
        assert_eq!(
            marker("python", &Ok(s)),
            "[rebody] lang=python files=2 edges -7 +9 calls_bound=3 refs_bound=1 names_ms=0.3 \
             refused=none"
        );
    }

    // ---- GA.7: Go ------------------------------------------------------------

    fn go_parse(path: &str, src: &str) -> FileParse {
        let qname = path.trim_end_matches(".go").replace('/', "::");
        glia_parser_go::parse_file(src, path, &qname, "example.com/app", repo()).expect("parse")
    }

    const GO_MODELS: &str = "\
package models

type Item struct {
	ID   string
	Name string
}

func (i *Item) Label() string {
	return i.Name
}

func NewItem(id string) *Item {
	return &Item{ID: id}
}

type Namer interface {
	Label() string
}

type Tagged struct {
	Item
	Tag string
}
";

    const GO_STORE: &str = "\
package store

import \"example.com/app/models\"

type Store struct {
	items []*models.Item
}

func New() *Store {
	return &Store{}
}

func (s *Store) Items() []*models.Item {
	return s.items
}
";

    /// Split receivers (LA.23d): `Store`'s methods declared beside it, and a
    /// struct embedding it below them.
    const GO_EXTRA: &str = "\
package store

import \"example.com/app/models\"

func (s *Store) Count() int {
	return len(s.Items())
}

func (s *Store) Add(id string) {
	s.items = append(s.items, models.NewItem(id))
}

type Cached struct {
	Store
	hits int
}
";

    const GO_SERVICE: &str = "\
package service

import (
	\"example.com/app/models\"
	\"example.com/app/store\"
)

var shared = store.New()

func helper() int {
	return 1
}

func Run() int {
	shared.Add(\"a\")
	for _, it := range shared.Items() {
		it.Label()
	}
	run := func() int { return helper() }
	c := &store.Cached{}
	c.Count()
	item := &models.Item{ID: \"x\"}
	mk := models.NewItem
	_ = mk
	_ = item
	return run()
}
";

    const GO_ROUTES: &str = "\
package routes

import \"github.com/gin-gonic/gin\"

func Register(r *gin.Engine) {
	r.GET(\"/items\", list)
}

func list(c *gin.Context) {
	c.String(200, \"ok\")
}
";

    const GO_MOUNT: &str = "\
package routes

import \"github.com/gin-gonic/gin\"

func Mount(r *gin.Engine) {
	v1 := r.Group(\"/v1\")
	sub(v1)
}

func sub(g *gin.RouterGroup) {
	g.GET(\"/x\", list)
}
";

    const GO_HANDLER: &str = "\
package api

import (
	\"net/http\"

	\"example.com/app/models\"
	\"github.com/gin-gonic/gin\"
)

func Show(c *gin.Context) {
	c.JSON(http.StatusOK, models.Item{})
}
";

    fn go_base() -> Vec<(&'static str, String)> {
        vec![
            ("api/handler.go", GO_HANDLER.to_string()),
            ("models/models.go", GO_MODELS.to_string()),
            ("routes/mount.go", GO_MOUNT.to_string()),
            ("routes/routes.go", GO_ROUTES.to_string()),
            ("service/service.go", GO_SERVICE.to_string()),
            ("store/extra.go", GO_EXTRA.to_string()),
            ("store/store.go", GO_STORE.to_string()),
        ]
    }

    fn go_parses(tree: &[(&'static str, String)]) -> Vec<FileParse> {
        tree.iter().map(|(p, s)| go_parse(p, s)).collect()
    }

    fn go_clean(tree: &[(&'static str, String)]) -> RepoGraph {
        let mut g = build_go(repo(), go_parses(tree)).expect("build");
        g.canonicalize();
        g
    }

    /// Re-stitch `files` (path, new source) on the base graph, compare with
    /// a clean build of the edited tree, and return the stats.
    fn go_rebody(files: &[(&str, String)], label: &str) -> RebodyStats {
        let tree = go_base();
        let mut g = go_clean(&tree);
        let mut after = tree.clone();
        for (path, src) in files {
            after = edited(&after, path, src);
        }
        let edits = files
            .iter()
            .map(|(p, s)| BodyEdit::new(go_parse(p, s)))
            .collect();
        let stats = rebuild_go_bodies(&mut g, edits, &go_parses(&after))
            .unwrap_or_else(|e| panic!("{label}: refused: {e}"));
        assert_same(&g, &go_clean(&after), label);
        stats
    }

    fn go_refused(path: &str, src: &str) -> Result<RebodyStats, Refused> {
        let tree = go_base();
        let mut g = go_clean(&tree);
        let before = g.clone();
        let after = edited(&tree, path, src);
        let got = rebuild_go_bodies(
            &mut g,
            vec![BodyEdit::new(go_parse(path, src))],
            &go_parses(&after),
        );
        assert!(got.is_err(), "{path}: {got:?}");
        assert_same(&g, &before, "refused leaves the graph");
        got
    }

    fn rule_count(g: &RepoGraph, emitter: &str, rule: &str) -> usize {
        g.edges
            .iter()
            .filter_map(evidence::Evidence::of)
            .filter(|ev| ev.emitter == emitter && ev.rule.as_deref() == Some(rule))
            .count()
    }

    /// The fixture reaches what a Go re-stitch must keep or re-derive: split
    /// receivers, embeds, implicit IMPLEMENTS, package-var and range-element
    /// receivers, value refs, a ROUTE, a mount and a wire cell.
    #[test]
    fn go_base_fixture_reaches_every_path() {
        let g = go_clean(&go_base());
        assert_eq!(rule_count(&g, "graph:go_packages", "split_receiver"), 2);
        let embeds = rule_count(&g, "graph:go_packages", "embed_package")
            + rule_count(&g, "graph:go_packages", "embed_import");
        assert_eq!(embeds, 2, "Tagged and Cached embed");
        assert!(
            g.edges
                .iter()
                .filter_map(evidence::Evidence::of)
                .any(|ev| ev.emitter == "graph:iface"),
            "an implicit IMPLEMENTS"
        );
        assert!(rule_count(&g, "graph:go_packages", "composite_literal") > 0);
        assert!(rule_count(&g, "graph:go_packages", "func_value") > 0);
        let kinds: Vec<_> = g.nav.kind_by_id.values().copied().collect();
        assert!(kinds.contains(&node_kind::ROUTE));
        let service = g.symbols.module_by_qname["service::service"];
        assert!(
            g.nav.local_types[&service].contains_key("shared"),
            "the package var is a MODULE-scope local"
        );
        let label = g
            .nav
            .qname_by_id
            .iter()
            .find(|(_, q)| q.as_str() == "models::models::Item::Label")
            .map(|(id, _)| *id)
            .expect("Label");
        assert!(
            g.edges
                .iter()
                .any(|e| e.category == edge_category::CALLS && e.to == label),
            "the range element's receiver binds Item.Label"
        );
    }

    #[test]
    fn go_a_typed_receiver_call_added() {
        let src = GO_SERVICE.replace(
            "\tshared.Add(\"a\")\n",
            "\tn := shared.Count()\n\t_ = n\n\tshared.Add(\"a\")\n",
        );
        let s = go_rebody(&[("service/service.go", src)], "typed receiver");
        assert_eq!(s.files, 1);
        assert!(s.calls_bound > 0 && s.edges_removed > 0, "{s:?}");
    }

    #[test]
    fn go_a_closure_call_changed() {
        let src = GO_SERVICE.replace(
            "\trun := func() int { return helper() }\n",
            "\trun := func() int {\n\t\tc.Count()\n\t\treturn helper() + 1\n\t}\n",
        );
        let src = src.replace("\tc := &store.Cached{}\n", "").replace(
            "\tshared.Add(\"a\")\n",
            "\tshared.Add(\"a\")\n\tc := &store.Cached{}\n",
        );
        go_rebody(&[("service/service.go", src)], "closure");
    }

    #[test]
    fn go_a_range_element_receiver_removed() {
        let src = GO_SERVICE.replace(
            "\tfor _, it := range shared.Items() {\n\t\tit.Label()\n\t}\n",
            "",
        );
        go_rebody(&[("service/service.go", src)], "range element");
    }

    #[test]
    fn go_a_composite_literal_and_func_value_moved() {
        let src = GO_SERVICE.replace(
            "\titem := &models.Item{ID: \"x\"}\n\tmk := models.NewItem\n",
            "\tvar pad1, pad2 int\n\t_, _ = pad1, pad2\n\tmk := models.NewItem\n\titem := []models.Item{{ID: \"y\"}}\n",
        );
        go_rebody(&[("service/service.go", src)], "composite literal");
    }

    /// A split method's body edit shifts the embedding struct below it: the
    /// embed's row is re-derived, the split DEFINES and IMPLEMENTS kept.
    #[test]
    fn go_a_split_method_body_shifts_an_embed() {
        let src = GO_EXTRA.replace(
            "\treturn len(s.Items())\n",
            "\titems := s.Items()\n\ttotal := len(items)\n\treturn total\n",
        );
        go_rebody(&[("store/extra.go", src)], "split method");
    }

    /// The struct's own file edited: its methods other files declare keep
    /// their DEFINES; the implicit IMPLEMENTS of a type in the file is kept.
    #[test]
    fn go_a_struct_file_and_an_implementer_edited() {
        let store = GO_STORE.replace(
            "\treturn &Store{}\n",
            "\ts := &Store{}\n\ts.items = nil\n\treturn s\n",
        );
        let models = GO_MODELS.replace(
            "\treturn i.Name\n",
            "\tif i == nil {\n\t\treturn \"\"\n\t}\n\treturn i.Name\n",
        );
        let s = go_rebody(
            &[("models/models.go", models), ("store/store.go", store)],
            "two files",
        );
        assert_eq!(s.files, 2);
    }

    #[test]
    fn go_a_route_file_is_refused() {
        let src = GO_ROUTES.replace("\tc.String(200, \"ok\")\n", "\tc.String(201, \"ok\")\n");
        let got = go_refused("routes/routes.go", &src);
        assert_eq!(got, Err(Refused::Unsupported("route")));
        assert!(
            marker("go", &got).ends_with("refused=unsupported:route"),
            "{}",
            marker("go", &got)
        );
    }

    #[test]
    fn go_a_mount_file_is_refused() {
        let src = GO_MOUNT.replace("\tsub(v1)\n", "\tsub(v1)\n\t_ = v1\n");
        assert_eq!(
            go_refused("routes/mount.go", &src),
            Err(Refused::Unsupported("mount"))
        );
    }

    #[test]
    fn go_a_wire_handler_is_refused() {
        let src = GO_HANDLER.replace(
            "\tc.JSON(http.StatusOK, models.Item{})\n",
            "\tc.Header(\"x\", \"y\")\n\tc.JSON(http.StatusOK, models.Item{})\n",
        );
        assert_eq!(
            go_refused("api/handler.go", &src),
            Err(Refused::Unsupported("wire"))
        );
    }

    #[test]
    fn go_a_func_added_is_refused() {
        let src = format!("{GO_EXTRA}\nfunc Extra() int {{\n\treturn 2\n}}\n");
        assert_eq!(
            go_refused("store/extra.go", &src),
            Err(Refused::SurfaceMoved("store/extra.go".to_string()))
        );
    }

    #[test]
    fn go_an_import_added_is_refused() {
        let src = GO_SERVICE.replace(
            "\t\"example.com/app/store\"\n",
            "\t\"example.com/app/store\"\n\t\"example.com/app/api\"\n",
        );
        assert!(matches!(
            go_refused("service/service.go", &src),
            Err(Refused::SurfaceMoved(_))
        ));
    }
}
