//! Per-language graph builders plus the shared merge / nav / symbol-table
//! passes they all run.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, ImportStmt, ImportTarget, UnresolvedRef,
    edge_category, node_kind,
};
use repo_graph_core::{Cell, Confidence, Edge, NodeId, RepoId};

use crate::calls::{
    emit_method_level_implements, enclosing_module, push_edge, resolve_calls, resolve_refs,
};
use crate::imports::{
    resolve_imports_go, resolve_imports_python, resolve_imports_slash, resolve_imports_ts,
};
use crate::rust_paths::{RustCrate, RustIndex, resolve_imports_rust};
use crate::types::{GraphError, RepoGraph, SymbolTable};

// ============================================================================
// Public entry point
// ============================================================================

/// Build a per-repo Python graph from a set of file-parse outputs.
pub fn build_python(repo: RepoId, parses: Vec<FileParse>) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    resolve_imports_python(&mut g, &all_imports);
    resolve_calls(&mut g, &all_calls, |_, _| None);
    resolve_refs(&mut g, &all_refs);
    emit_method_level_implements(&mut g);
    Ok(g)
}

/// Build a per-repo Go graph. The engine gives every Go file its own MODULE
/// (the qname is the file path); a package is the set of files in one
/// directory.
///
/// LA.23d: a method declared in a different file from its receiver struct is
/// re-parented under that struct first ([`bind_split_go_receivers`]), so
/// `class_methods` holds it (SelfMethod and field-typed calls resolve) and
/// its file's `module_symbols` no longer lists it as a top-level function.
/// Its Bare / Attribute calls still resolve in its own file's scope
/// ([`resolve_go_calls`]).
///
/// LD.7b: an interface's embedded interfaces bind package-scoped and to an
/// INTERFACE only ([`resolve_go_embeds`]); then, Go interfaces being
/// satisfied implicitly, [`emit_go_implicit_implements`] derives each type ->
/// interface IMPLEMENTS edge from method names, before A6.6 pairs them
/// method by method.
///
/// LA.13b: a package is a directory, so an import binds the imported
/// directory and a call every generic lookup missed resolves across the
/// package's files ([`GoPackages`]), with no qname or persisted-table change.
pub fn build_go(repo: RepoId, parses: Vec<FileParse>) -> Result<RepoGraph, GraphError> {
    let (g, split, implicit, packages) = build_go_passes(repo, parses);
    if let Some(line) = split.marker() {
        eprintln!("{line}");
    }
    if let Some(stats) = implicit {
        eprintln!("{}", stats.marker());
    }
    if let Some(line) = packages.marker() {
        eprintln!("{line}");
    }
    Ok(g)
}

/// [`build_go`]'s passes, returning the stats its markers print.
fn build_go_passes(
    repo: RepoId,
    parses: Vec<FileParse>,
) -> (RepoGraph, SplitStats, Option<GoImplicitStats>, GoPackageStats) {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    let split = bind_split_go_receivers(&mut g);
    build_symbol_table(&mut g);
    let packages = GoPackages::build(&g, &all_imports);
    let dir_bound_imports = resolve_imports_go(&mut g, &all_imports, &packages);
    resolve_go_calls(&mut g, &all_calls, &split, |g, site| packages.resolve(g, site));
    let package_stats = packages.stats(dir_bound_imports);
    let (embeds, refs): (Vec<UnresolvedRef>, Vec<UnresolvedRef>) =
        all_refs.into_iter().partition(|r| is_go_embed(&g.nav, r));
    resolve_refs(&mut g, &refs);
    resolve_go_embeds(&mut g, &embeds, &all_imports);
    let implicit = emit_go_implicit_implements(&mut g);
    emit_method_level_implements(&mut g);
    (g, split, implicit, package_stats)
}

/// Build a per-repo TypeScript graph. TS import sources are raw strings
/// (`./user`, `@angular/core`) that the caller resolves to module qnames via
/// `resolve_source`. Returning `None` treats the import as external (no edge).
pub fn build_typescript<R>(
    repo: RepoId,
    parses: Vec<FileParse>,
    resolve_source: R,
) -> Result<RepoGraph, GraphError>
where
    R: Fn(&str, &str) -> Option<String>,
{
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    resolve_imports_ts(&mut g, &all_imports, &resolve_source);
    resolve_calls(&mut g, &all_calls, |_, _| None);
    resolve_refs(&mut g, &all_refs);
    emit_method_level_implements(&mut g);
    Ok(g)
}

/// Build a per-repo graph for languages whose import paths are dotted
/// (`foo.bar.Baz`) or already normalised to `::` form. Reuses the Python
/// resolver because `.replace('.', "::")` is a no-op on already-`::` paths.
/// Covers Java, Kotlin, C#, PHP, Scala, Clojure, Elixir; Rust has
/// [`build_rust`].
pub fn build_dotted(repo: RepoId, parses: Vec<FileParse>) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    resolve_imports_python(&mut g, &all_imports);
    resolve_calls(&mut g, &all_calls, |_, _| None);
    resolve_refs(&mut g, &all_refs);
    emit_method_level_implements(&mut g);
    Ok(g)
}

/// Build a per-repo Rust graph. Same passes as [`build_dotted`], plus the
/// Rust path resolver in `resolve_calls`' `extra_hook` seam: a path call the
/// generic pass misses (`crate::a::f()`, `super::f()`, `Self::f()`,
/// `other_crate::f()`) is resolved against the module tree and `crates`, the
/// Cargo packages the walk found (LA.1a, [`crate::rust_paths`]).
///
/// LA.3: a Bare call inside an inline `mod x { .. }` first tries the
/// enclosing mods' own fns, innermost first (the generic order would bind a
/// same-named file-level fn), and the enum-variant USES refs `resolve_refs`
/// leaves are bound through the same path rules.
///
/// LA.1b: `use` trees resolve as Rust paths ([`resolve_imports_rust`], in
/// place of `resolve_imports_python`): workspace-crate uses, `pub use`
/// re-exports, aliases, globs and fn-body uses bind. A fn's own `use`s and
/// every glob stay in the returned bindings, which the pre-pass (a fn's
/// `use`s shadow the file's) and the hook (the file's globs, last) read.
pub fn build_rust(
    repo: RepoId,
    parses: Vec<FileParse>,
    crates: &[RustCrate],
) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    let idx = RustIndex::build(&g, crates);
    let bindings = resolve_imports_rust(&mut g, &all_imports, &idx);
    // Scoped pre-pass: a hit is a CALLS edge now; a miss (and every other
    // site) keeps its original position for the generic pass.
    let mut rest: Vec<CallSite> = Vec::with_capacity(all_calls.len());
    let mut mod_scoped = 0usize;
    for site in all_calls {
        match idx.resolve_scoped_bare(&g, &site, &bindings) {
            Some((to, mod_item)) => {
                push_edge(&mut g, site.from, to, edge_category::CALLS);
                mod_scoped += usize::from(mod_item);
            }
            None => rest.push(site),
        }
    }
    resolve_calls(&mut g, &rest, |g, site| {
        idx.resolve_call(g, site, &bindings)
    });
    resolve_refs(&mut g, &all_refs);
    idx.resolve_leftover_refs(&mut g, &bindings);
    emit_method_level_implements(&mut g);
    idx.report();
    idx.report_items(&g, mod_scoped);
    Ok(g)
}

/// Build a per-repo graph for Ruby. `require 'foo/bar'` imports carry a
/// slash-delimited path; convert to `::` then resolve against the module
/// table the Go-style way.
pub fn build_ruby(repo: RepoId, parses: Vec<FileParse>) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    resolve_imports_slash(&mut g, &all_imports);
    resolve_calls(&mut g, &all_calls, |_, _| None);
    resolve_refs(&mut g, &all_refs);
    emit_method_level_implements(&mut g);
    Ok(g)
}

// ============================================================================
// Shared merge: multi-file modules with the same NodeId collapse — their cells
// stack on a single Module node (TS re-exports, or Go files when a caller
// passes one qname per package, as tests/go_smoke does; the engine gives each
// Go file its own MODULE, see [`GoPackages`]).
// ============================================================================

fn merge_parses(
    repo: RepoId,
    parses: Vec<FileParse>,
) -> (
    RepoGraph,
    Vec<ImportStmt>,
    Vec<CallSite>,
    Vec<UnresolvedRef>,
) {
    let mut g = RepoGraph {
        repo,
        nodes: Vec::new(),
        edges: Vec::new(),
        nav: CodeNav::default(),
        symbols: SymbolTable::default(),
        unresolved_calls: Vec::new(),
        unresolved_refs: Vec::new(),
        properties: HashSet::new(),
    };

    let mut all_imports: Vec<ImportStmt> = Vec::new();
    let mut all_calls: Vec<CallSite> = Vec::new();
    let mut all_refs: Vec<UnresolvedRef> = Vec::new();
    let mut index: HashMap<NodeId, usize> = HashMap::new();

    for p in parses {
        for n in p.nodes {
            if let Some(&idx) = index.get(&n.id) {
                // Duplicate NodeId — append cells onto the existing node.
                append_cells(&mut g.nodes[idx].cells, n.cells);
            } else {
                index.insert(n.id, g.nodes.len());
                g.nodes.push(n);
            }
        }
        g.edges.extend(p.edges);
        merge_nav(&mut g.nav, p.nav);
        all_imports.extend(p.imports);
        all_calls.extend(p.calls);
        all_refs.extend(p.refs);
        g.properties.extend(p.properties);
    }

    // LB.3a: fold framework role overlays (SERVICE / COMPONENT / HOOK / ...)
    // into their same-qname declaration BEFORE any builder builds its symbol
    // table, so module symbols, INJECTS and CALLS bind to the declaration and
    // never to an edgeless marker. Every `build_*` goes through here.
    let stats = crate::roles::fold_role_overlays(&mut g, &mut all_calls, &mut all_refs);
    if stats.saw_role_nodes() {
        eprintln!("{}", stats.marker());
    }

    (g, all_imports, all_calls, all_refs)
}

fn append_cells(existing: &mut Vec<Cell>, incoming: Vec<Cell>) {
    existing.extend(incoming);
}

// ============================================================================
// Nav merge
// ============================================================================

fn merge_nav(dst: &mut CodeNav, src: CodeNav) {
    dst.name_by_id.extend(src.name_by_id);
    dst.qname_by_id.extend(src.qname_by_id);
    dst.kind_by_id.extend(src.kind_by_id);
    dst.parent_of.extend(src.parent_of);
    for (k, v) in src.children_of {
        dst.children_of.entry(k).or_default().extend(v);
    }
    // A6.2a: per-owner merge, so a partial class split across files keeps
    // every file's declared fields.
    for (owner, fields) in src.field_types {
        dst.field_types.entry(owner).or_default().extend(fields);
    }
}

// ============================================================================
// Symbol table
// ============================================================================

fn build_symbol_table(g: &mut RepoGraph) {
    for (id, qname) in &g.nav.qname_by_id {
        if g.nav.kind_by_id.get(id) == Some(&node_kind::MODULE) {
            g.symbols.module_by_qname.insert(qname.clone(), *id);
        }
    }

    // module_symbols: for each module, bare name → node id for its top-level defs.
    // Walk children_of; if parent kind == MODULE, child goes in module_symbols.
    for (parent, children) in &g.nav.children_of {
        let parent_kind = g.nav.kind_by_id.get(parent).copied();
        // MODULE (file) and PACKAGE (namespace, e.g. C# `namespace Shop.Services`)
        // both scope top-level type/fn defs. Registering PACKAGE children lets a
        // namespace-scoped type resolve by name (Pattern E INJECTS: C# services
        // live under a PACKAGE node, not a MODULE). A type indexed under both its
        // file MODULE and its namespace PACKAGE is deduped by id in the global
        // lookups, so this doesn't create false ambiguity.
        if parent_kind == Some(node_kind::MODULE) || parent_kind == Some(node_kind::PACKAGE) {
            let entry = g.symbols.module_symbols.entry(*parent).or_default();
            for child in children {
                if let Some(name) = g.nav.name_by_id.get(child) {
                    entry.insert(name.clone(), *child);
                }
            }
        } else if parent_kind == Some(node_kind::CLASS)
            || parent_kind == Some(node_kind::STRUCT)
            || parent_kind == Some(node_kind::ENUM)
        {
            // An ENUM owns its METHOD children exactly like a CLASS / STRUCT
            // (LA.30a: Rust `impl Enum`, Swift, Java / TS enum bodies). Its
            // ATTRIBUTE members are NOT indexed here — methods only.
            let entry = g.symbols.class_methods.entry(*parent).or_default();
            for child in children {
                if let Some(name) = g.nav.name_by_id.get(child)
                    && g.nav.kind_by_id.get(child) == Some(&node_kind::METHOD)
                {
                    entry.insert(name.clone(), *child);
                }
            }
        } else if parent_kind == Some(node_kind::INTERFACE) {
            // A6.6: an INTERFACE's METHOD children (C# / Java interface
            // members, Rust trait fns) go in their OWN table, never
            // `class_methods`: `unique_global_method` scans `class_methods`
            // for the HANDLED_BY fallback, and a second `GetById` there would
            // make a previously-unique handler ambiguous and delete its edge.
            // An interface with no METHOD child gets no entry.
            for child in children {
                if let Some(name) = g.nav.name_by_id.get(child)
                    && g.nav.kind_by_id.get(child) == Some(&node_kind::METHOD)
                {
                    g.symbols
                        .interface_methods
                        .entry(*parent)
                        .or_default()
                        .insert(name.clone(), *child);
                }
            }
        }
    }
}

// ============================================================================
// Go split-file receivers (LA.23d)
// ============================================================================

/// What [`bind_split_go_receivers`] did to one Go graph.
#[derive(Debug, Default)]
struct SplitStats {
    /// Re-parented methods in `g.nodes` order: `(method, its file MODULE,
    /// its STRUCT)`.
    bound: Vec<(NodeId, NodeId, NodeId)>,
    /// Receiver name declared by two or more types in the method's directory
    /// (a broken tree, or a package and its `_test` twin sharing a name).
    ambiguous: usize,
    /// Receiver name no STRUCT in the method's directory declares: a named
    /// non-struct type (`type Status int`, which the parser emits no node
    /// for), a lone INTERFACE, or a type outside the parsed tree.
    unmatched: usize,
}

impl SplitStats {
    /// `[go-recv] split-file methods bound: B (ambiguous=A unmatched=U)`, once
    /// per Go graph with any candidate method.
    fn marker(&self) -> Option<String> {
        (self.bound.len() + self.ambiguous + self.unmatched > 0).then(|| {
            format!(
                "[go-recv] split-file methods bound: {} (ambiguous={} unmatched={})",
                self.bound.len(),
                self.ambiguous,
                self.unmatched
            )
        })
    }
}

/// The receiver segment of a `visit_method` qname under its file MODULE:
/// `service::UserService::Get` under `service` -> `UserService`. Any other
/// shape -> `None`.
fn go_receiver_of<'a>(method_qname: &'a str, module_qname: &str) -> Option<&'a str> {
    let rest = method_qname.strip_prefix(module_qname)?.strip_prefix("::")?;
    let (recv, name) = rest.split_once("::")?;
    (!recv.is_empty() && !name.is_empty() && !name.contains("::")).then_some(recv)
}

/// Re-parent every Go METHOD the parser left under its file MODULE under its
/// receiver STRUCT. `visit_method` only sees the types of its own file, so a
/// method whose struct is declared in another file of the package falls back
/// to the MODULE.
///
/// A Go package is a directory and a MODULE qname is the file path, so a
/// node's package is its MODULE's qname minus the last `::` segment
/// (`svc::users::store` -> `svc::users`). A candidate's receiver is the
/// second-to-last segment of its qname ([`go_receiver_of`]). Exactly one
/// STRUCT / INTERFACE of that name in the directory, and that one a STRUCT,
/// binds: nav `parent_of` / `children_of` move to the struct (appended in
/// `g.nodes` order) and a struct -> method DEFINES edge is added, the
/// same-file shape. The file's module -> method DEFINES edge stays, since the
/// file still defines the method. Two same-named types in one directory bind
/// nothing: a tree that does not compile, or a package and its `_test` twin
/// (same directory) that both declare the name, as grpc-go's per-package
/// `type s struct` test suites do. The package clause is not in the parse,
/// so the directory cannot tell the two apart.
///
/// Runs before [`build_symbol_table`], which then indexes the method in
/// `class_methods[struct]` and leaves it out of its file's `module_symbols`.
/// Deterministic: candidates are visited in `g.nodes` order; the index is a
/// lookup table only.
fn bind_split_go_receivers(g: &mut RepoGraph) -> SplitStats {
    let mut stats = SplitStats::default();
    let mut binds: Vec<(NodeId, NodeId, NodeId)> = Vec::new();
    {
        let nav = &g.nav;
        let module_of = |id: &NodeId| -> Option<(NodeId, &str)> {
            let parent = *nav.parent_of.get(id)?;
            if nav.kind_by_id.get(&parent) != Some(&node_kind::MODULE) {
                return None;
            }
            Some((parent, nav.qname_by_id.get(&parent)?.as_str()))
        };
        let mut types: HashMap<(&str, &str), Vec<NodeId>> = HashMap::new();
        let mut candidates: Vec<(NodeId, NodeId, &str, &str)> = Vec::new();
        for n in &g.nodes {
            let Some(&kind) = nav.kind_by_id.get(&n.id) else { continue };
            let Some((module, module_qname)) = module_of(&n.id) else { continue };
            if kind == node_kind::STRUCT || kind == node_kind::INTERFACE {
                if let Some(name) = nav.name_by_id.get(&n.id) {
                    let ids = types.entry((go_package_dir(module_qname), name.as_str())).or_default();
                    if !ids.contains(&n.id) {
                        ids.push(n.id);
                    }
                }
            } else if kind == node_kind::METHOD
                && let Some(recv) = nav
                    .qname_by_id
                    .get(&n.id)
                    .and_then(|q| go_receiver_of(q, module_qname))
            {
                candidates.push((n.id, module, go_package_dir(module_qname), recv));
            }
        }
        for (method, module, dir, recv) in candidates {
            match types.get(&(dir, recv)).map(Vec::as_slice) {
                Some(&[only]) if nav.kind_by_id.get(&only) == Some(&node_kind::STRUCT) => {
                    binds.push((method, module, only));
                }
                Some([_, _, ..]) => stats.ambiguous += 1,
                _ => stats.unmatched += 1,
            }
        }
    }
    for &(method, module, strukt) in &binds {
        g.nav.parent_of.insert(method, strukt);
        if let Some(kids) = g.nav.children_of.get_mut(&module) {
            kids.retain(|k| *k != method);
        }
        g.nav.children_of.entry(strukt).or_default().push(method);
        push_edge(g, strukt, method, edge_category::DEFINES);
    }
    stats.bound = binds;
    stats
}

/// Go call resolution around [`bind_split_go_receivers`]. A bound method's
/// SelfMethod and receiver-field calls need its struct as their owner, which
/// the re-parented nav gives them. Its Bare and Attribute calls name the
/// package-level functions and imports of its OWN file (Go imports are per
/// file), and `resolve_calls` scopes them by walking to the nearest MODULE,
/// which through the struct is the struct's file. So those sites resolve in a
/// second pass with every bound method pointed back at its file MODULE; the
/// struct parents are restored after it. With no bound method this is one
/// `resolve_calls` over every site, as before LA.23d.
///
/// `hook` is the `extra_hook` of both passes ([`GoPackages::resolve`],
/// LA.13b). It scopes a site by its nearest MODULE too, so in the second pass
/// a bound method's Bare call reaches the other files of its OWN file's
/// package.
fn resolve_go_calls<H>(g: &mut RepoGraph, calls: &[CallSite], split: &SplitStats, hook: H)
where
    H: Fn(&RepoGraph, &CallSite) -> Option<NodeId> + Copy,
{
    if split.bound.is_empty() {
        resolve_calls(g, calls, hook);
        return;
    }
    let bound: HashSet<NodeId> = split.bound.iter().map(|&(m, _, _)| m).collect();
    let (file_scoped, rest): (Vec<CallSite>, Vec<CallSite>) =
        calls.iter().cloned().partition(|s| {
            matches!(s.qualifier, CallQualifier::Bare(_) | CallQualifier::Attribute { .. })
                && under_bound_method(&g.nav, &bound, s.from)
        });
    resolve_calls(g, &rest, hook);
    for &(method, module, _) in &split.bound {
        g.nav.parent_of.insert(method, module);
    }
    resolve_calls(g, &file_scoped, hook);
    for &(method, _, strukt) in &split.bound {
        g.nav.parent_of.insert(method, strukt);
    }
}

/// True when `id` is in `bound` or sits under a node that is. The walk is
/// bounded by the nav's size, so a malformed parent cycle ends in `false`.
fn under_bound_method(nav: &CodeNav, bound: &HashSet<NodeId>, mut id: NodeId) -> bool {
    for _ in 0..=nav.parent_of.len() {
        if bound.contains(&id) {
            return true;
        }
        match nav.parent_of.get(&id) {
            Some(&parent) => id = parent,
            None => return false,
        }
    }
    false
}

/// A Go node's package: a package is a directory and a MODULE qname is the
/// file path, so it is the MODULE qname minus its last `::` segment
/// (`svc::users::store` -> `svc::users`, a root-level file -> `""`).
fn go_package_dir(module_qname: &str) -> &str {
    module_qname.rsplit_once("::").map_or("", |(dir, _)| dir)
}

// ============================================================================
// Go package = directory (LA.13b)
// ============================================================================

/// The Go packages of one graph, used only while resolving. A Go package is
/// a directory, but every Go file is its own MODULE (the engine passes the
/// file path as the qname: `internal/store/store.go` ->
/// `internal::store::store`), so on its own the generic pass sees one file
/// where Go sees the package: an import binds a single file of the imported
/// package, and a Bare call only finds a def in the caller's own file.
///
/// Neither qnames nor the persisted `module_symbols` change: one MODULE per
/// package would stack every file's cells onto one node and change every Go
/// id, and copying the package's symbols into each file's table would store
/// a 50-file package's table 50 times. Instead [`resolve_imports_go`] binds an
/// import of a directory to one of its files ([`GoPackages::import_target`]),
/// and [`GoPackages::resolve`], `resolve_calls`' `extra_hook`, searches the
/// rest of the package for a call every generic lookup missed. An edge that
/// resolved before still resolves to the same node.
///
/// A `_test.go` file (a MODULE whose last segment ends in `_test`) is built
/// only by `go test`: an importer never sees it and a non-test file never
/// calls into it. It may also be an external `package x_test`, which the
/// directory cannot tell apart (the package clause is not in the parse).
///
/// Deterministic: member lists are sorted by qname and a lookup binds only
/// when exactly one node answers, never the first of several.
pub(crate) struct GoPackages {
    /// Package dir -> its file MODULEs, sorted by qname.
    by_dir: HashMap<String, Vec<NodeId>>,
    /// Package dir -> its file named after the dir (Go's `store/store.go`
    /// convention), when it has one.
    dir_named: HashMap<String, NodeId>,
    /// File MODULE -> its package dir.
    dir_of: HashMap<NodeId, String>,
    /// The `_test.go` file MODULEs.
    tests: HashSet<NodeId>,
    /// Importing file MODULE -> local package name -> import path, for every
    /// `ImportTarget::Module` except a blank (`_`) or dot (`.`) import.
    import_path: HashMap<NodeId, HashMap<String, String>>,
    /// Calls [`GoPackages::resolve`] bound.
    sibling_calls: std::cell::Cell<usize>,
}

/// Where an import of a package directory binds ([`GoPackages::import_target`]).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DirImport {
    /// No Go file sits in exactly that directory.
    NoDir,
    /// The directory's only file is the importer itself (an external
    /// `x_test` package importing `x` whose other files are not parsed).
    OnlyImporter,
    /// The file the IMPORTS edge and binding point at.
    Bound(NodeId),
}

/// What [`GoPackages`] did to one Go graph, for the `[go-package]` marker.
#[derive(Debug, Default, PartialEq, Eq)]
struct GoPackageStats {
    /// Package directories (a root-level file's package is `""`).
    dirs: usize,
    /// Directories holding two or more files.
    multi_file: usize,
    /// Imports bound by the package-directory step of
    /// [`resolve_imports_go`].
    dir_bound_imports: usize,
    /// Calls bound in another file of a package by [`GoPackages::resolve`].
    sibling_calls: usize,
}

impl GoPackageStats {
    /// `[go-package] dirs=D multi_file=M dir_bound_imports=I sibling_calls=S`,
    /// once per Go graph with any MODULE.
    fn marker(&self) -> Option<String> {
        (self.dirs > 0).then(|| {
            format!(
                "[go-package] dirs={} multi_file={} dir_bound_imports={} sibling_calls={}",
                self.dirs, self.multi_file, self.dir_bound_imports, self.sibling_calls
            )
        })
    }
}

impl GoPackages {
    /// Index the MODULEs of `g` (after [`build_symbol_table`]) by package
    /// directory, and the imports by importing file and local name.
    fn build(g: &RepoGraph, imports: &[ImportStmt]) -> Self {
        let mut modules: Vec<(&str, NodeId)> =
            g.symbols.module_by_qname.iter().map(|(q, id)| (q.as_str(), *id)).collect();
        modules.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let mut by_dir: HashMap<String, Vec<NodeId>> = HashMap::new();
        let mut dir_named: HashMap<String, NodeId> = HashMap::new();
        let mut dir_of: HashMap<NodeId, String> = HashMap::new();
        let mut tests: HashSet<NodeId> = HashSet::new();
        for (qname, id) in modules {
            let dir = go_package_dir(qname);
            let stem = qname.rsplit("::").next().unwrap_or(qname);
            by_dir.entry(dir.to_string()).or_default().push(id);
            dir_of.insert(id, dir.to_string());
            if stem.ends_with("_test") {
                tests.insert(id);
            } else if !dir.is_empty() && dir.rsplit("::").next() == Some(stem) {
                dir_named.insert(dir.to_string(), id);
            }
        }
        let mut import_path: HashMap<NodeId, HashMap<String, String>> = HashMap::new();
        for stmt in imports {
            let ImportTarget::Module { path, alias } = &stmt.target else {
                continue;
            };
            let local = match alias.as_deref() {
                Some("_") | Some(".") => continue,
                Some(a) => a,
                None => path.rsplit("::").next().unwrap_or(path),
            };
            if let Some(&from) = g.symbols.module_by_qname.get(&stmt.from_module) {
                import_path.entry(from).or_default().insert(local.to_string(), path.clone());
            }
        }
        GoPackages {
            by_dir,
            dir_named,
            dir_of,
            tests,
            import_path,
            sibling_calls: std::cell::Cell::new(0),
        }
    }

    /// The file an import of package directory `dir` by file `importer`
    /// binds: the dir-named file, else the first non-test file by qname, else
    /// the first test file; never the importer itself.
    pub(crate) fn import_target(&self, dir: &str, importer: NodeId) -> DirImport {
        let Some(members) = self.by_dir.get(dir) else {
            return DirImport::NoDir;
        };
        let others = || members.iter().copied().filter(move |&m| m != importer);
        self.dir_named
            .get(dir)
            .copied()
            .filter(|&m| m != importer)
            .or_else(|| others().find(|m| !self.tests.contains(m)))
            .or_else(|| others().next())
            .map_or(DirImport::OnlyImporter, DirImport::Bound)
    }

    /// `resolve_calls`' `extra_hook`, consulted only after every generic
    /// lookup missed:
    ///
    /// * `Bare(name)`: the def `name` of another file in the caller's package
    ///   (a package's top-level names are one scope in Go). A test file's
    ///   defs answer only a test file.
    /// * `Attribute { base, name }` with `base` an import of the caller's
    ///   file: the exported def `name` of a non-test file of the imported
    ///   package (a test file of the same directory also sees its test
    ///   files, the `export_test.go` pattern). That package is the import
    ///   path when the tree has that directory; else, for an import the tail
    ///   fallback bound, the bound file's directory. An import bound to a
    ///   file whose qname IS the path (a file `a/b.go` for an import of `a/b`,
    ///   with no `a/b/` directory) is not widened: that file is not the
    ///   imported package.
    ///
    /// A METHOD never answers (Go calls one only through a value), nor do
    /// `init` (several per package, never callable) or `_`. Two answers (a
    /// build-tag pair, a package and its `_test` twin) bind nothing.
    fn resolve(&self, g: &RepoGraph, site: &CallSite) -> Option<NodeId> {
        let module = enclosing_module(&g.nav, site.from)?;
        let from_test = self.tests.contains(&module);
        let hit = match &site.qualifier {
            CallQualifier::Bare(name) => {
                self.unique_in(g, self.dir_of.get(&module)?, name, module, from_test)
            }
            CallQualifier::Attribute { base, name }
                if name.chars().next().is_some_and(char::is_uppercase) =>
            {
                let dir = self.imported_dir(g, module, base)?;
                let same_dir = self.dir_of.get(&module).is_some_and(|d| d == dir);
                self.unique_in(g, dir, name, module, from_test && same_dir)
            }
            _ => None,
        }?;
        self.sibling_calls.set(self.sibling_calls.get() + 1);
        Some(hit)
    }

    /// The package directory `module`'s import bound as `base` stands for
    /// (see [`GoPackages::resolve`]).
    fn imported_dir(&self, g: &RepoGraph, module: NodeId, base: &str) -> Option<&str> {
        let path = self.import_path.get(&module)?.get(base)?;
        if let Some((dir, _)) = self.by_dir.get_key_value(path) {
            return Some(dir);
        }
        let bound = *g.symbols.module_import_bindings.get(&module)?.get(base)?;
        if g.nav.kind_by_id.get(&bound) != Some(&node_kind::MODULE)
            || g.nav.qname_by_id.get(&bound) == Some(path)
        {
            return None;
        }
        self.dir_of.get(&bound).map(String::as_str)
    }

    /// The one non-METHOD top-level def `name` across the files of `dir`
    /// other than `caller`, test files included only `with_tests`.
    fn unique_in(
        &self,
        g: &RepoGraph,
        dir: &str,
        name: &str,
        caller: NodeId,
        with_tests: bool,
    ) -> Option<NodeId> {
        if name == "init" || name == "_" {
            return None;
        }
        let mut hit: Option<NodeId> = None;
        for &m in self.by_dir.get(dir)? {
            if m == caller || (!with_tests && self.tests.contains(&m)) {
                continue;
            }
            let Some(&id) = g.symbols.module_symbols.get(&m).and_then(|s| s.get(name)) else {
                continue;
            };
            if g.nav.kind_by_id.get(&id) == Some(&node_kind::METHOD) {
                continue;
            }
            match hit {
                Some(existing) if existing == id => {}
                Some(_) => return None,
                None => hit = Some(id),
            }
        }
        hit
    }

    fn stats(&self, dir_bound_imports: usize) -> GoPackageStats {
        GoPackageStats {
            dirs: self.by_dir.len(),
            multi_file: self.by_dir.values().filter(|m| m.len() > 1).count(),
            dir_bound_imports,
            sibling_calls: self.sibling_calls.get(),
        }
    }
}

// ============================================================================
// Go implicit interface satisfaction (LD.7b)
// ============================================================================

/// An embed ref of the Go parser: INHERITS_FROM out of an INTERFACE (one
/// `type_elem` naming a single type).
fn is_go_embed(nav: &CodeNav, r: &UnresolvedRef) -> bool {
    r.category == edge_category::INHERITS_FROM
        && nav.kind_by_id.get(&r.from) == Some(&node_kind::INTERFACE)
}

/// Bind each Go embed ref to the INTERFACE it names, Go's way: `R` is the
/// interface `R` of the embedding interface's own package (directory), and
/// `pkg.R` the interface `R` of the package the file imports as `pkg` (its
/// import path, or a directory ending in it when the path has two or more
/// segments, for a go.mod below the repo root). Exactly one INTERFACE must
/// match, else the ref stays in `unresolved_refs`: another module's
/// interface (`io.Reader`), a predeclared one (`error`, `comparable`), a
/// constraint's exact type term (`interface{ MyStruct }`), or an ambiguous
/// name.
///
/// Not `resolve_refs`: its repo-wide by-name fallback binds any same-named
/// node, and on grpc-go bound `ServerStream` (a same-package interface) to a
/// DATA_ENTITY of that name. Edges are pushed in ref (parse) order.
fn resolve_go_embeds(g: &mut RepoGraph, embeds: &[UnresolvedRef], imports: &[ImportStmt]) {
    if embeds.is_empty() {
        return;
    }
    let mut bound: Vec<(NodeId, NodeId)> = Vec::new();
    let mut unbound: Vec<UnresolvedRef> = Vec::new();
    {
        let nav = &g.nav;
        let mut by_dir_name: HashMap<(&str, &str), Vec<NodeId>> = HashMap::new();
        let mut by_name: HashMap<&str, Vec<(&str, NodeId)>> = HashMap::new();
        for n in &g.nodes {
            if nav.kind_by_id.get(&n.id) != Some(&node_kind::INTERFACE) {
                continue;
            }
            let (Some(name), Some(parent)) = (nav.name_by_id.get(&n.id), nav.parent_of.get(&n.id)) else {
                continue;
            };
            let Some(module_qname) = nav.qname_by_id.get(parent) else {
                continue;
            };
            let dir = go_package_dir(module_qname);
            by_dir_name.entry((dir, name.as_str())).or_default().push(n.id);
            by_name.entry(name.as_str()).or_default().push((dir, n.id));
        }
        // (importing file's MODULE qname, local package name) -> import path.
        let mut import_paths: HashMap<(&str, &str), &str> = HashMap::new();
        for stmt in imports {
            let ImportTarget::Module { path, alias } = &stmt.target else {
                continue;
            };
            let local = match alias.as_deref() {
                Some("_") | Some(".") => continue,
                Some(a) => a,
                None => path.rsplit("::").next().unwrap_or(path),
            };
            import_paths.insert((stmt.from_module.as_str(), local), path.as_str());
        }
        let only = |ids: &[NodeId]| match ids {
            [one] => Some(*one),
            _ => None,
        };
        for r in embeds {
            let module_qname = nav.qname_by_id.get(&r.from_module).map_or("", String::as_str);
            let hit = match &r.qualifier {
                CallQualifier::Bare(name) => by_dir_name
                    .get(&(go_package_dir(module_qname), name.as_str()))
                    .and_then(|ids| only(ids)),
                CallQualifier::Attribute { base, name } => {
                    import_paths.get(&(module_qname, base.as_str())).and_then(|&path| {
                        let suffix = format!("::{path}");
                        let ids: Vec<NodeId> = by_name
                            .get(name.as_str())
                            .into_iter()
                            .flatten()
                            .filter(|(dir, _)| {
                                *dir == path || (path.contains("::") && dir.ends_with(&suffix))
                            })
                            .map(|&(_, id)| id)
                            .collect();
                        only(&ids)
                    })
                }
                _ => None,
            };
            match hit {
                Some(to) if to != r.from => bound.push((r.from, to)),
                _ => unbound.push(r.clone()),
            }
        }
    }
    for (from, to) in bound {
        push_edge(g, from, to, edge_category::INHERITS_FROM);
    }
    g.unresolved_refs.extend(unbound);
}

/// Method sets of the predeclared interfaces a Go interface can embed. No
/// parse declares them, so their embed ref stays unresolved; this is what
/// they contribute instead of leaving the embedding interface's set unknown.
const GO_PREDECLARED_IFACES: &[(&str, &[&str])] = &[("error", &["Error"])];

/// What [`emit_go_implicit_implements`] did to one Go graph.
#[derive(Debug, Default, PartialEq, Eq)]
struct GoImplicitStats {
    /// IMPLEMENTS edges pushed.
    edges: usize,
    /// Interfaces with a known, non-empty method set: the ones matched.
    interfaces: usize,
    /// Distinct types given at least one IMPLEMENTS edge.
    types: usize,
    /// Distinct interface -> interface INHERITS_FROM edges (embeds) followed.
    embedded: usize,
    /// Interfaces not matched because an embed did not bind, so their method
    /// set is not fully known.
    open: usize,
}

impl GoImplicitStats {
    /// `[iface] go implicit implements: E (interfaces=I types=T embedded=M
    /// open=O)`, once per Go graph that has an INTERFACE.
    fn marker(&self) -> String {
        format!(
            "[iface] go implicit implements: {} (interfaces={} types={} embedded={} open={})",
            self.edges, self.interfaces, self.types, self.embedded, self.open
        )
    }
}

/// Go satisfies interfaces implicitly: a named type implements an interface
/// when its method NAME set covers the interface's (own + embedded,
/// transitively). Signatures are not compared (the parser records none), so
/// every edge is `Confidence::Medium`.
///
/// * An interface's own methods are its `interface_methods` (the parser's
///   `method_elem` METHOD children). Its embedded interfaces are its
///   INHERITS_FROM edges to another INTERFACE, walked depth-first with a
///   visited set, so an embedding cycle (which Go rejects) cannot loop.
/// * An embed that did not become such an edge leaves the set unknown and
///   the interface is skipped (`open`): an INHERITS_FROM ref still in
///   `unresolved_refs` (another module's `io.Reader`, `comparable`) or an edge
///   to a non-INTERFACE. The predeclared `error` is the exception and
///   contributes `Error` ([`GO_PREDECLARED_IFACES`]). Matching on the known
///   part instead would pair `type ReadCloser interface { io.Reader; Close()
///   error }` with every type that has a `Close`.
/// * An empty set (`interface{}`, a constraint of type terms only) is
///   implemented by nothing here.
/// * An unexported method name is package-scoped in Go: it matches only a
///   type in the package (directory, [`go_package_dir`]) of the interface
///   that declares it.
/// * Types are the STRUCT / CLASS owners in `class_methods`. Pointer and
///   value receivers both count toward a type's set, and methods promoted
///   from an embedded struct field are not seen.
///
/// Runs after [`resolve_go_embeds`] (the embed refs are bound) and before
/// `emit_method_level_implements`, which then pairs each new edge's methods.
/// Pairs are sorted by id and deduped before any edge is pushed (the method
/// tables are HashMaps with per-process seeds, and edge order feeds the
/// store's shard hashes), and an IMPLEMENTS edge already present is not
/// pushed again. `None` when the graph has no INTERFACE.
fn emit_go_implicit_implements(g: &mut RepoGraph) -> Option<GoImplicitStats> {
    let ifaces: Vec<NodeId> = g
        .nodes
        .iter()
        .map(|n| n.id)
        .filter(|id| g.nav.kind_by_id.get(id) == Some(&node_kind::INTERFACE))
        .collect();
    if ifaces.is_empty() {
        return None;
    }
    let mut stats = GoImplicitStats::default();
    let mut pairs: Vec<(NodeId, NodeId)> = Vec::new();
    {
        let nav = &g.nav;
        let is_iface = |id: &NodeId| nav.kind_by_id.get(id) == Some(&node_kind::INTERFACE);
        let pkg_of = |id: &NodeId| -> Option<&str> {
            let parent = nav.parent_of.get(id)?;
            if nav.kind_by_id.get(parent) != Some(&node_kind::MODULE) {
                return None;
            }
            nav.qname_by_id.get(parent).map(|q| go_package_dir(q))
        };

        let mut embeds: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut embed_pairs: HashSet<(NodeId, NodeId)> = HashSet::new();
        let mut open: HashSet<NodeId> = HashSet::new();
        for e in &g.edges {
            if e.category != edge_category::INHERITS_FROM || !is_iface(&e.from) {
                continue;
            }
            if !is_iface(&e.to) {
                open.insert(e.from);
            } else if embed_pairs.insert((e.from, e.to)) {
                embeds.entry(e.from).or_default().push(e.to);
            }
        }
        stats.embedded = embed_pairs.len();
        let mut predeclared: HashMap<NodeId, Vec<&'static str>> = HashMap::new();
        for r in &g.unresolved_refs {
            if r.category != edge_category::INHERITS_FROM || !is_iface(&r.from) {
                continue;
            }
            let known = match &r.qualifier {
                CallQualifier::Bare(name) => {
                    GO_PREDECLARED_IFACES.iter().find(|(n, _)| n == name).map(|(_, ms)| *ms)
                }
                _ => None,
            };
            match known {
                Some(ms) => predeclared.entry(r.from).or_default().extend(ms.iter().copied()),
                None => {
                    open.insert(r.from);
                }
            }
        }

        // Method name -> the types declaring it, to narrow each interface's
        // candidates to the holders of its rarest method name.
        let mut by_method: HashMap<&str, Vec<NodeId>> = HashMap::new();
        for (owner, methods) in &g.symbols.class_methods {
            let kind = nav.kind_by_id.get(owner);
            if kind != Some(&node_kind::STRUCT) && kind != Some(&node_kind::CLASS) {
                continue;
            }
            for name in methods.keys() {
                by_method.entry(name.as_str()).or_default().push(*owner);
            }
        }

        for &iface in &ifaces {
            // (method name, declaring package when the name is unexported).
            let mut set: HashSet<(&str, Option<&str>)> = HashSet::new();
            let mut visited: HashSet<NodeId> = HashSet::new();
            let mut stack = vec![iface];
            let mut known = true;
            'walk: while let Some(i) = stack.pop() {
                if !visited.insert(i) {
                    continue;
                }
                if open.contains(&i) {
                    known = false;
                    break;
                }
                for name in g.symbols.interface_methods.get(&i).into_iter().flat_map(|m| m.keys()) {
                    if name.chars().next().is_some_and(char::is_uppercase) {
                        set.insert((name.as_str(), None));
                    } else if let Some(pkg) = pkg_of(&i) {
                        set.insert((name.as_str(), Some(pkg)));
                    } else {
                        known = false;
                        break 'walk;
                    }
                }
                for &name in predeclared.get(&i).into_iter().flatten() {
                    set.insert((name, None));
                }
                stack.extend(embeds.get(&i).into_iter().flatten().copied());
            }
            if !known {
                stats.open += 1;
                continue;
            }
            if set.is_empty() {
                continue;
            }
            stats.interfaces += 1;
            let Some(candidates) = set
                .iter()
                .map(|(name, _)| by_method.get(name).map_or(&[][..], Vec::as_slice))
                .min_by_key(|c| c.len())
            else {
                continue;
            };
            for &ty in candidates {
                let Some(methods) = g.symbols.class_methods.get(&ty) else {
                    continue;
                };
                let covers = set.iter().all(|&(name, pkg)| {
                    methods.contains_key(name) && pkg.is_none_or(|p| pkg_of(&ty) == Some(p))
                });
                if covers {
                    pairs.push((ty, iface));
                }
            }
        }
    }
    pairs.sort_unstable_by_key(|(a, b)| (a.0, b.0));
    pairs.dedup();
    let existing: HashSet<(NodeId, NodeId)> = g
        .edges
        .iter()
        .filter(|e| e.category == edge_category::IMPLEMENTS)
        .map(|e| (e.from, e.to))
        .collect();
    pairs.retain(|p| !existing.contains(p));
    stats.types = pairs.iter().map(|&(ty, _)| ty).collect::<HashSet<_>>().len();
    stats.edges = pairs.len();
    for (from, to) in pairs {
        g.edges.push(Edge { from, to, category: edge_category::IMPLEMENTS, confidence: Confidence::Medium });
    }
    Some(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::repo;
    use repo_graph_code_domain::{GRAPH_TYPE, ImportTarget, edge_category};
    use repo_graph_core::{Confidence, Node};

    #[test]
    fn empty_repo_builds_cleanly() {
        let g = build_python(repo(), vec![]).unwrap();
        assert!(g.nodes.is_empty());
        assert!(g.edges.is_empty());
    }

    #[test]
    fn build_dotted_resolves_java_style_imports() {
        // Two modules: com::foo (imports com::bar::Helper) and com::bar.
        let repo = repo();
        let foo_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "com::foo");
        let bar_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "com::bar");
        let helper_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CLASS, "com::bar::Helper");

        let mut foo_nav = CodeNav::default();
        foo_nav.record(foo_id, "foo", "com::foo", node_kind::MODULE, None);
        let foo = FileParse {
            nodes: vec![Node { id: foo_id, repo, confidence: Confidence::Strong, cells: vec![] }],
            edges: vec![],
            imports: vec![ImportStmt {
                from_module: "com::foo".to_string(),
                target: ImportTarget::Symbol {
                    module: "com::bar".to_string(),
                    name: "Helper".to_string(),
                    alias: None,
                    level: 0,
                },
            }],
            calls: vec![],
            refs: vec![],
            nav: foo_nav,
            properties: HashSet::new(),
        };

        let mut bar_nav = CodeNav::default();
        bar_nav.record(bar_id, "bar", "com::bar", node_kind::MODULE, None);
        bar_nav.record(helper_id, "Helper", "com::bar::Helper", node_kind::CLASS, Some(bar_id));
        let bar = FileParse {
            nodes: vec![
                Node { id: bar_id, repo, confidence: Confidence::Strong, cells: vec![] },
                Node { id: helper_id, repo, confidence: Confidence::Strong, cells: vec![] },
            ],
            edges: vec![],
            imports: vec![],
            calls: vec![],
            refs: vec![],
            nav: bar_nav,
            properties: HashSet::new(),
        };

        let g = build_dotted(repo, vec![foo, bar]).unwrap();
        assert!(
            g.edges.iter().any(|e|
                e.from == foo_id && e.to == bar_id && e.category == edge_category::IMPORTS
            ),
            "expected IMPORTS edge from com::foo to com::bar"
        );
        let foo_bindings = g.symbols.module_import_bindings.get(&foo_id).unwrap();
        assert_eq!(foo_bindings.get("Helper").copied(), Some(helper_id));
    }

    #[test]
    fn build_ruby_resolves_slash_requires() {
        let repo = repo();
        let app_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "app");
        let foo_bar_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "foo::bar");

        let mut app_nav = CodeNav::default();
        app_nav.record(app_id, "app", "app", node_kind::MODULE, None);
        let app = FileParse {
            nodes: vec![Node { id: app_id, repo, confidence: Confidence::Strong, cells: vec![] }],
            edges: vec![],
            imports: vec![ImportStmt {
                from_module: "app".to_string(),
                target: ImportTarget::Module {
                    path: "foo/bar".to_string(),
                    alias: None,
                },
            }],
            calls: vec![],
            refs: vec![],
            nav: app_nav,
            properties: HashSet::new(),
        };

        let mut foo_bar_nav = CodeNav::default();
        foo_bar_nav.record(foo_bar_id, "bar", "foo::bar", node_kind::MODULE, None);
        let foo_bar = FileParse {
            nodes: vec![Node { id: foo_bar_id, repo, confidence: Confidence::Strong, cells: vec![] }],
            edges: vec![],
            imports: vec![],
            calls: vec![],
            refs: vec![],
            nav: foo_bar_nav,
            properties: HashSet::new(),
        };

        let g = build_ruby(repo, vec![app, foo_bar]).unwrap();
        assert!(
            g.edges.iter().any(|e|
                e.from == app_id && e.to == foo_bar_id && e.category == edge_category::IMPORTS
            ),
            "expected IMPORTS edge from app to foo::bar (slash → ::)"
        );
    }

    // ---- A6.6: interface method table + method-level IMPLEMENTS -------------

    /// C# shape: `interface IUserService { GetById(); }` +
    /// `class UserService : IUserService { GetById(); Load(); }`, the class-level
    /// heritage arriving as a Bare IMPLEMENTS ref (bound by resolve_refs).
    /// Returns (file, iface, iface_get, impl_cls, impl_get, impl_load).
    fn iface_and_impl() -> (FileParse, [NodeId; 5]) {
        let r = repo();
        let m = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, "svc");
        let iface = NodeId::from_parts(GRAPH_TYPE, r, node_kind::INTERFACE, "svc::IUserService");
        let iface_get =
            NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, "svc::IUserService::GetById");
        let cls = NodeId::from_parts(GRAPH_TYPE, r, node_kind::CLASS, "svc::UserService");
        let cls_get =
            NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, "svc::UserService::GetById");
        let cls_load =
            NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, "svc::UserService::Load");
        let mut nav = CodeNav::default();
        nav.record(m, "svc", "svc", node_kind::MODULE, None);
        nav.record(iface, "IUserService", "svc::IUserService", node_kind::INTERFACE, Some(m));
        nav.record(iface_get, "GetById", "svc::IUserService::GetById", node_kind::METHOD, Some(iface));
        nav.record(cls, "UserService", "svc::UserService", node_kind::CLASS, Some(m));
        nav.record(cls_get, "GetById", "svc::UserService::GetById", node_kind::METHOD, Some(cls));
        nav.record(cls_load, "Load", "svc::UserService::Load", node_kind::METHOD, Some(cls));
        let node = |id| Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] };
        let file = FileParse {
            nodes: [m, iface, iface_get, cls, cls_get, cls_load].into_iter().map(node).collect(),
            edges: vec![],
            imports: vec![],
            calls: vec![],
            refs: vec![UnresolvedRef {
                from: cls,
                from_module: m,
                qualifier: repo_graph_code_domain::CallQualifier::Bare("IUserService".to_string()),
                category: edge_category::IMPLEMENTS,
            }],
            nav,
            properties: HashSet::new(),
        };
        (file, [iface, iface_get, cls, cls_get, cls_load])
    }

    fn implements(g: &RepoGraph) -> Vec<(NodeId, NodeId)> {
        g.edges
            .iter()
            .filter(|e| e.category == edge_category::IMPLEMENTS)
            .map(|e| (e.from, e.to))
            .collect()
    }

    /// The INTERFACE's methods land in their own table and never in
    /// `class_methods`, which `unique_global_method` scans for HANDLED_BY.
    #[test]
    fn interface_methods_are_not_in_class_methods() {
        let (file, [iface, iface_get, cls, cls_get, _]) = iface_and_impl();
        let g = build_dotted(repo(), vec![file]).unwrap();
        assert!(!g.symbols.class_methods.contains_key(&iface));
        assert_eq!(g.symbols.interface_methods[&iface].get("GetById").copied(), Some(iface_get));
        assert_eq!(g.symbols.class_methods[&cls].get("GetById").copied(), Some(cls_get));
        assert!(!g.symbols.interface_methods.contains_key(&cls));
    }

    /// Class-level `UserService -> IUserService` pairs the same-named method:
    /// `UserService::GetById -> IUserService::GetById`. `Load` has no interface
    /// counterpart and pairs with nothing.
    #[test]
    fn method_level_implements_pairs_same_named_methods() {
        let (file, [iface, iface_get, cls, cls_get, _]) = iface_and_impl();
        let g = build_dotted(repo(), vec![file]).unwrap();
        assert_eq!(implements(&g), vec![(cls, iface), (cls_get, iface_get)]);
    }

    /// Every builder runs the pass, and a second run adds nothing: an
    /// IMPLEMENTS pair already present is never pushed twice.
    #[test]
    fn method_level_implements_is_idempotent_and_runs_in_every_builder() {
        for build in [build_python, build_go, build_dotted, build_ruby] {
            let (file, [_, iface_get, _, cls_get, _]) = iface_and_impl();
            let mut g = build(repo(), vec![file]).unwrap();
            assert!(implements(&g).contains(&(cls_get, iface_get)));
            let before = g.edges.len();
            crate::calls::emit_method_level_implements(&mut g);
            assert_eq!(g.edges.len(), before, "re-running the pass must not duplicate");
        }
        let (file, [_, iface_get, _, cls_get, _]) = iface_and_impl();
        let g = build_typescript(repo(), vec![file], |_, _| None).unwrap();
        assert!(implements(&g).contains(&(cls_get, iface_get)));
    }

    /// Two implementors of one interface, several methods: the emitted edge
    /// order is a pure function of the ids (sorted), not of HashMap seeds.
    #[test]
    fn method_level_implements_order_is_sorted() {
        let r = repo();
        let m = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, "svc");
        let iface = NodeId::from_parts(GRAPH_TYPE, r, node_kind::INTERFACE, "svc::I");
        let mut nav = CodeNav::default();
        nav.record(m, "svc", "svc", node_kind::MODULE, None);
        nav.record(iface, "I", "svc::I", node_kind::INTERFACE, Some(m));
        let mut ids = vec![m, iface];
        let mut edges = vec![];
        for name in ["a", "b", "c", "d"] {
            let q = format!("svc::I::{name}");
            let id = NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, &q);
            nav.record(id, name, &q, node_kind::METHOD, Some(iface));
            ids.push(id);
        }
        for cls_name in ["X", "Y"] {
            let cq = format!("svc::{cls_name}");
            let cls = NodeId::from_parts(GRAPH_TYPE, r, node_kind::CLASS, &cq);
            nav.record(cls, cls_name, &cq, node_kind::CLASS, Some(m));
            ids.push(cls);
            edges.push(repo_graph_core::Edge {
                from: cls,
                to: iface,
                category: edge_category::IMPLEMENTS,
                confidence: Confidence::Strong,
            });
            for name in ["a", "b", "c", "d"] {
                let q = format!("{cq}::{name}");
                let id = NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, &q);
                nav.record(id, name, &q, node_kind::METHOD, Some(cls));
                ids.push(id);
            }
        }
        let node = |id| Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] };
        let file = FileParse {
            nodes: ids.into_iter().map(node).collect(),
            edges,
            imports: vec![],
            calls: vec![],
            refs: vec![],
            nav,
            properties: HashSet::new(),
        };
        let g = build_dotted(r, vec![file]).unwrap();
        let method_level: Vec<_> = implements(&g).into_iter().filter(|(_, to)| *to != iface).collect();
        assert_eq!(method_level.len(), 8, "2 implementors x 4 methods");
        let mut sorted = method_level.clone();
        sorted.sort_unstable_by_key(|(a, b)| (a.0, b.0));
        assert_eq!(method_level, sorted);
    }

    /// The HANDLED_BY guard: a route handler `h.GetById` is unique among
    /// CLASS / STRUCT methods even though the interface declares `GetById`
    /// too. Were interface methods merged into `class_methods`, the lookup
    /// would turn ambiguous and the HANDLED_BY edge would vanish.
    #[test]
    fn interface_method_does_not_make_handled_by_ambiguous() {
        let (mut file, [_, _, _, cls_get, _]) = iface_and_impl();
        let r = repo();
        let m = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, "svc");
        let route = NodeId::from_parts(GRAPH_TYPE, r, node_kind::ROUTE, "svc::GET /users/{id}");
        file.nav.record(route, "GET /users/{id}", "svc::GET /users/{id}", node_kind::ROUTE, Some(m));
        file.nodes.push(Node { id: route, repo: r, confidence: Confidence::Strong, cells: vec![] });
        file.refs.push(UnresolvedRef {
            from: route,
            from_module: m,
            qualifier: repo_graph_code_domain::CallQualifier::Attribute {
                base: "h".to_string(),
                name: "GetById".to_string(),
            },
            category: edge_category::HANDLED_BY,
        });
        let g = build_dotted(r, vec![file]).unwrap();
        let handled: Vec<_> = g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::HANDLED_BY)
            .map(|e| (e.from, e.to))
            .collect();
        assert_eq!(handled, vec![(route, cls_get)]);
    }

    // ---- LA.23d: Go split-file receivers ------------------------------------

    fn gid(kind: repo_graph_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    /// One Go file shaped the way the Go parser emits it: MODULE `module` plus
    /// `(kind, qname, parent qname)` items, where a `None` parent is the
    /// MODULE (a METHOD there is one whose receiver type this file does not
    /// declare) and `Some(q)` names an earlier item of the same file. Every
    /// item gets its parent -> item DEFINES edge.
    fn go_file(
        module: &str,
        items: &[(repo_graph_core::NodeKindId, &str, Option<&str>)],
    ) -> FileParse {
        let r = repo();
        let m = gid(node_kind::MODULE, module);
        let mut nav = CodeNav::default();
        nav.record(m, module.rsplit("::").next().unwrap_or(module), module, node_kind::MODULE, None);
        let mut ids: HashMap<&str, NodeId> = HashMap::new();
        let mut nodes = vec![Node { id: m, repo: r, confidence: Confidence::Strong, cells: vec![] }];
        let mut edges = vec![];
        for &(kind, qname, parent) in items {
            let id = gid(kind, qname);
            let parent_id = parent.map_or(m, |p| ids[p]);
            let name = qname.rsplit("::").next().unwrap_or(qname);
            nav.record(id, name, qname, kind, Some(parent_id));
            nodes.push(Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] });
            edges.push(repo_graph_core::Edge {
                from: parent_id,
                to: id,
                category: edge_category::DEFINES,
                confidence: Confidence::Strong,
            });
            ids.insert(qname, id);
        }
        FileParse {
            nodes,
            edges,
            imports: vec![],
            calls: vec![],
            refs: vec![],
            nav,
            properties: HashSet::new(),
        }
    }

    fn has_edge(g: &RepoGraph, from: NodeId, to: NodeId, category: repo_graph_core::EdgeCategoryId) -> bool {
        g.edges.iter().any(|e| e.from == from && e.to == to && e.category == category)
    }

    /// The fixtures/go-split-receiver shape: `types.go` declares `UserRepo`
    /// (with `Find`) and `UserService { repo *UserRepo }`; `service.go`
    /// declares `(s *UserService) Get` calling `s.audit(id)` and
    /// `s.repo.Find(id)`, and `audit`.
    fn fixture_shape() -> Vec<FileParse> {
        let mut types = go_file(
            "types",
            &[
                (node_kind::STRUCT, "types::UserRepo", None),
                (node_kind::METHOD, "types::UserRepo::Find", Some("types::UserRepo")),
                (node_kind::STRUCT, "types::UserService", None),
            ],
        );
        types.nav.record_field_type(gid(node_kind::STRUCT, "types::UserService"), "repo", "UserRepo");
        let mut service = go_file(
            "service",
            &[
                (node_kind::METHOD, "service::UserService::Get", None),
                (node_kind::METHOD, "service::UserService::audit", None),
            ],
        );
        let get = gid(node_kind::METHOD, "service::UserService::Get");
        service.calls = vec![
            CallSite { from: get, qualifier: CallQualifier::SelfMethod("audit".to_string()) },
            CallSite {
                from: get,
                qualifier: CallQualifier::ComplexReceiver {
                    receiver: "self.repo".to_string(),
                    name: "Find".to_string(),
                },
            },
        ];
        vec![types, service]
    }

    /// A method whose struct lives in another file of the package moves under
    /// the struct: nav parent / children, a struct -> method DEFINES next to
    /// the kept module -> method one, `class_methods` instead of
    /// `module_symbols`, and its self-call and field-typed call both bind.
    #[test]
    fn split_method_rebinds_under_its_struct_and_resolves_calls() {
        let (svc_struct, find) =
            (gid(node_kind::STRUCT, "types::UserService"), gid(node_kind::METHOD, "types::UserRepo::Find"));
        let (service, get, audit) = (
            gid(node_kind::MODULE, "service"),
            gid(node_kind::METHOD, "service::UserService::Get"),
            gid(node_kind::METHOD, "service::UserService::audit"),
        );

        let (mut g, _, _, _) = merge_parses(repo(), fixture_shape());
        let stats = bind_split_go_receivers(&mut g);
        assert_eq!(stats.bound, vec![(get, service, svc_struct), (audit, service, svc_struct)]);
        assert_eq!(
            stats.marker().as_deref(),
            Some("[go-recv] split-file methods bound: 2 (ambiguous=0 unmatched=0)")
        );

        let g = build_go(repo(), fixture_shape()).unwrap();
        assert_eq!(g.nav.parent_of[&get], svc_struct);
        assert_eq!(g.nav.parent_of[&audit], svc_struct);
        assert_eq!(g.nav.children_of[&svc_struct], vec![get, audit]);
        assert!(!g.nav.children_of[&service].contains(&get));
        for m in [get, audit] {
            assert!(has_edge(&g, svc_struct, m, edge_category::DEFINES), "struct -> method DEFINES");
            assert!(has_edge(&g, service, m, edge_category::DEFINES), "the file still defines it");
        }
        assert_eq!(g.symbols.class_methods[&svc_struct].get("Get").copied(), Some(get));
        assert!(g.symbols.module_symbols.get(&service).is_none_or(|s| !s.contains_key("Get")));
        assert!(has_edge(&g, get, audit, edge_category::CALLS), "SelfMethod across files");
        assert!(has_edge(&g, get, find, edge_category::CALLS), "field-typed call across files");
        assert!(g.unresolved_calls.is_empty());
    }

    /// Package identity is the directory: a receiver `T` binds the `T` of its
    /// own directory, never a same-named `T` of another package, and a
    /// directory with no `T` binds nothing.
    #[test]
    fn same_named_struct_in_another_package_is_not_bound() {
        let parses = vec![
            go_file("a::types", &[(node_kind::STRUCT, "a::types::T", None)]),
            go_file("b::types", &[(node_kind::STRUCT, "b::types::T", None)]),
            go_file("b::svc", &[(node_kind::METHOD, "b::svc::T::M", None)]),
            go_file("c::svc", &[(node_kind::METHOD, "c::svc::T::M", None)]),
        ];
        let (mut g, _, _, _) = merge_parses(repo(), parses);
        let stats = bind_split_go_receivers(&mut g);
        let (b_m, c_m) = (gid(node_kind::METHOD, "b::svc::T::M"), gid(node_kind::METHOD, "c::svc::T::M"));
        assert_eq!(stats.bound, vec![(b_m, gid(node_kind::MODULE, "b::svc"), gid(node_kind::STRUCT, "b::types::T"))]);
        assert_eq!((stats.ambiguous, stats.unmatched), (0, 1));
        assert_eq!(g.nav.parent_of[&c_m], gid(node_kind::MODULE, "c::svc"));
        assert!(g.nav.children_of.get(&gid(node_kind::STRUCT, "a::types::T")).is_none_or(|k| k.is_empty()));
    }

    /// Two files of one directory each declaring `type T struct` (a tree that
    /// does not compile) leave a third file's `T` method where it was.
    #[test]
    fn ambiguous_receiver_type_binds_nothing() {
        let parses = vec![
            go_file("x::one", &[(node_kind::STRUCT, "x::one::T", None)]),
            go_file("x::two", &[(node_kind::STRUCT, "x::two::T", None)]),
            go_file("x::three", &[(node_kind::METHOD, "x::three::T::M", None)]),
        ];
        let (mut g, _, _, _) = merge_parses(repo(), parses);
        let before = g.edges.len();
        let stats = bind_split_go_receivers(&mut g);
        assert!(stats.bound.is_empty());
        assert_eq!((stats.ambiguous, stats.unmatched), (1, 0));
        assert_eq!(
            stats.marker().as_deref(),
            Some("[go-recv] split-file methods bound: 0 (ambiguous=1 unmatched=0)")
        );
        assert_eq!(g.edges.len(), before);
        assert_eq!(g.nav.parent_of[&gid(node_kind::METHOD, "x::three::T::M")], gid(node_kind::MODULE, "x::three"));
    }

    /// A method declared beside its struct is already under it: not a
    /// candidate, no edge added, no marker.
    #[test]
    fn same_file_method_is_untouched() {
        let parses = vec![go_file(
            "s",
            &[(node_kind::STRUCT, "s::T", None), (node_kind::METHOD, "s::T::M", Some("s::T"))],
        )];
        let (mut g, _, _, _) = merge_parses(repo(), parses);
        let (edges, parent) = (g.edges.len(), g.nav.parent_of.clone());
        let stats = bind_split_go_receivers(&mut g);
        assert!(stats.bound.is_empty());
        assert_eq!(stats.marker(), None);
        assert_eq!(g.edges.len(), edges);
        assert_eq!(g.nav.parent_of, parent);
    }

    /// Go imports and package-level functions are per file, so a bound
    /// method's Bare / Attribute calls keep resolving in its OWN file
    /// (`handlers.go`), not the struct's (`server.go`, which imports nothing),
    /// while its self-call binds through the struct. A bare `Handle()` from a
    /// function of that file no longer lands on the method.
    #[test]
    fn split_method_bare_and_import_calls_keep_their_file_scope() {
        let mut handlers = go_file(
            "app::handlers",
            &[
                (node_kind::METHOD, "app::handlers::Server::Handle", None),
                (node_kind::METHOD, "app::handlers::Server::helper", None),
                (node_kind::FUNCTION, "app::handlers::writeJSON", None),
                (node_kind::FUNCTION, "app::handlers::run", None),
            ],
        );
        handlers.imports = vec![ImportStmt {
            from_module: "app::handlers".to_string(),
            target: ImportTarget::Module { path: "store".to_string(), alias: None },
        }];
        let (handle, helper, write_json, run) = (
            gid(node_kind::METHOD, "app::handlers::Server::Handle"),
            gid(node_kind::METHOD, "app::handlers::Server::helper"),
            gid(node_kind::FUNCTION, "app::handlers::writeJSON"),
            gid(node_kind::FUNCTION, "app::handlers::run"),
        );
        let site = |from, qualifier| CallSite { from, qualifier };
        handlers.calls = vec![
            site(handle, CallQualifier::Bare("writeJSON".to_string())),
            site(
                handle,
                CallQualifier::Attribute { base: "store".to_string(), name: "Validate".to_string() },
            ),
            site(handle, CallQualifier::SelfMethod("helper".to_string())),
            site(run, CallQualifier::Bare("Handle".to_string())),
        ];
        let parses = vec![
            go_file("store::store", &[(node_kind::FUNCTION, "store::store::Validate", None)]),
            go_file("app::server", &[(node_kind::STRUCT, "app::server::Server", None)]),
            handlers,
        ];
        let g = build_go(repo(), parses).unwrap();
        let server = gid(node_kind::STRUCT, "app::server::Server");
        assert_eq!(g.nav.parent_of[&handle], server, "struct parent restored after the file pass");
        assert!(has_edge(&g, handle, write_json, edge_category::CALLS), "same-file bare call");
        assert!(
            has_edge(&g, handle, gid(node_kind::FUNCTION, "store::store::Validate"), edge_category::CALLS),
            "import of the method's own file"
        );
        assert!(has_edge(&g, handle, helper, edge_category::CALLS), "self-call through the struct");
        assert!(!g.edges.iter().any(|e| e.from == run && e.category == edge_category::CALLS));
        assert_eq!(g.unresolved_calls.len(), 1, "only the bare `Handle()` stays unresolved");
    }

    // ---- LD.7b: Go implicit interface satisfaction --------------------------

    /// An embed ref the Go parser emits: INHERITS_FROM out of `iface` (an
    /// INTERFACE of `module`).
    fn embed(module: &str, iface: &str, qualifier: CallQualifier) -> UnresolvedRef {
        UnresolvedRef {
            from: gid(node_kind::INTERFACE, iface),
            from_module: gid(node_kind::MODULE, module),
            qualifier,
            category: edge_category::INHERITS_FROM,
        }
    }

    /// `build_go` with the implicit pass's stats.
    fn go_implicit(parses: Vec<FileParse>) -> (RepoGraph, Option<GoImplicitStats>) {
        let (g, _, stats, _) = build_go_passes(repo(), parses);
        (g, stats)
    }

    fn implements_edge(g: &RepoGraph, from: NodeId, to: NodeId) -> Option<Confidence> {
        g.edges
            .iter()
            .find(|e| e.from == from && e.to == to && e.category == edge_category::IMPLEMENTS)
            .map(|e| e.confidence)
    }

    /// The fixtures/go-implicit-iface shape: `store.go` declares `Store { Get;
    /// Put }`, `mem.go` declares `MemStore` with both methods and `ReadOnly`
    /// with `Get` only.
    fn implicit_iface_shape() -> Vec<FileParse> {
        vec![
            go_file(
                "store",
                &[
                    (node_kind::INTERFACE, "store::Store", None),
                    (node_kind::METHOD, "store::Store::Get", Some("store::Store")),
                    (node_kind::METHOD, "store::Store::Put", Some("store::Store")),
                    (node_kind::FUNCTION, "store::Use", None),
                ],
            ),
            go_file(
                "mem",
                &[
                    (node_kind::STRUCT, "mem::MemStore", None),
                    (node_kind::METHOD, "mem::MemStore::Get", Some("mem::MemStore")),
                    (node_kind::METHOD, "mem::MemStore::Put", Some("mem::MemStore")),
                    (node_kind::STRUCT, "mem::ReadOnly", None),
                    (node_kind::METHOD, "mem::ReadOnly::Get", Some("mem::ReadOnly")),
                ],
            ),
        ]
    }

    /// A type implements an interface only when it declares every one of its
    /// methods; the edge is Medium (names, not signatures), and A6.6 then
    /// pairs the methods of the new edge.
    #[test]
    fn go_implicit_implements_requires_full_method_set() {
        let (store, mem_store, read_only) = (
            gid(node_kind::INTERFACE, "store::Store"),
            gid(node_kind::STRUCT, "mem::MemStore"),
            gid(node_kind::STRUCT, "mem::ReadOnly"),
        );
        let (_, stats) = go_implicit(implicit_iface_shape());
        assert_eq!(
            stats.map(|s| s.marker()).as_deref(),
            Some("[iface] go implicit implements: 1 (interfaces=1 types=1 embedded=0 open=0)")
        );

        let g = build_go(repo(), implicit_iface_shape()).unwrap();
        assert_eq!(implements_edge(&g, mem_store, store), Some(Confidence::Medium));
        assert_eq!(implements_edge(&g, read_only, store), None, "ReadOnly lacks Put");
        for name in ["Get", "Put"] {
            let (from, to) = (
                gid(node_kind::METHOD, &format!("mem::MemStore::{name}")),
                gid(node_kind::METHOD, &format!("store::Store::{name}")),
            );
            assert!(implements_edge(&g, from, to).is_some(), "method-level {name}");
        }
        let (ro_get, store_get) =
            (gid(node_kind::METHOD, "mem::ReadOnly::Get"), gid(node_kind::METHOD, "store::Store::Get"));
        assert_eq!(implements_edge(&g, ro_get, store_get), None);
    }

    /// Several interfaces and implementors: the whole edge Vec is identical
    /// across builds (the method tables' HashMap seeds differ per map).
    #[test]
    fn go_implicit_implements_is_deterministic() {
        let shape = || {
            let mut items: Vec<(repo_graph_core::NodeKindId, String, Option<String>)> = Vec::new();
            for i in ["A", "B", "C", "D"] {
                let q = format!("pkg::I{i}");
                items.push((node_kind::INTERFACE, q.clone(), None));
                for m in ["Open", "Close", "Read"] {
                    items.push((node_kind::METHOD, format!("{q}::{m}{i}"), Some(q.clone())));
                }
            }
            for t in ["W", "X", "Y", "Z"] {
                let q = format!("pkg::{t}");
                items.push((node_kind::STRUCT, q.clone(), None));
                for i in ["A", "B", "C", "D"] {
                    for m in ["Open", "Close", "Read"] {
                        items.push((node_kind::METHOD, format!("{q}::{m}{i}"), Some(q.clone())));
                    }
                }
            }
            let borrowed: Vec<_> =
                items.iter().map(|(k, q, p)| (*k, q.as_str(), p.as_deref())).collect();
            vec![go_file("pkg", &borrowed)]
        };
        let first = build_go(repo(), shape()).unwrap();
        let second = build_go(repo(), shape()).unwrap();
        let implicit: Vec<_> = first
            .edges
            .iter()
            .filter(|e| e.category == edge_category::IMPLEMENTS && e.confidence == Confidence::Medium)
            .collect();
        assert_eq!(implicit.len(), 16, "4 types x 4 interfaces");
        assert_eq!(first.edges, second.edges);
    }

    /// The fixtures/go-iface-embed shape: `Store` embeds `Reader`, so its set
    /// is {Get, Put}; `Mem` has both and implements both interfaces, `Half`
    /// has only `Put` and implements neither.
    #[test]
    fn go_embedded_interface_method_set_is_transitive() {
        let mut file = go_file(
            "store",
            &[
                (node_kind::INTERFACE, "store::Reader", None),
                (node_kind::METHOD, "store::Reader::Get", Some("store::Reader")),
                (node_kind::INTERFACE, "store::Store", None),
                (node_kind::METHOD, "store::Store::Put", Some("store::Store")),
                (node_kind::STRUCT, "store::Mem", None),
                (node_kind::METHOD, "store::Mem::Get", Some("store::Mem")),
                (node_kind::METHOD, "store::Mem::Put", Some("store::Mem")),
                (node_kind::STRUCT, "store::Half", None),
                (node_kind::METHOD, "store::Half::Put", Some("store::Half")),
            ],
        );
        file.refs = vec![embed("store", "store::Store", CallQualifier::Bare("Reader".to_string()))];
        let (reader, store, mem, half) = (
            gid(node_kind::INTERFACE, "store::Reader"),
            gid(node_kind::INTERFACE, "store::Store"),
            gid(node_kind::STRUCT, "store::Mem"),
            gid(node_kind::STRUCT, "store::Half"),
        );
        let (g, stats) = go_implicit(vec![file]);
        assert!(has_edge(&g, store, reader, edge_category::INHERITS_FROM));
        assert_eq!(implements_edge(&g, mem, store), Some(Confidence::Medium));
        assert_eq!(implements_edge(&g, mem, reader), Some(Confidence::Medium));
        assert_eq!(implements_edge(&g, half, store), None, "Half lacks the embedded Get");
        assert_eq!(implements_edge(&g, half, reader), None);
        assert_eq!(
            stats.map(|s| s.marker()).as_deref(),
            Some("[iface] go implicit implements: 2 (interfaces=2 types=1 embedded=1 open=0)")
        );
    }

    /// An embed that does not bind (`io.Reader` from outside the parse)
    /// leaves the interface's method set unknown: no type implements it on
    /// its own `Close` alone. The predeclared `error` is known: `Error`.
    #[test]
    fn go_unbound_embed_leaves_interface_unmatched() {
        let mut file = go_file(
            "port",
            &[
                (node_kind::INTERFACE, "port::ReadCloser", None),
                (node_kind::METHOD, "port::ReadCloser::Close", Some("port::ReadCloser")),
                (node_kind::INTERFACE, "port::CodedError", None),
                (node_kind::METHOD, "port::CodedError::Code", Some("port::CodedError")),
                (node_kind::STRUCT, "port::File", None),
                (node_kind::METHOD, "port::File::Close", Some("port::File")),
                (node_kind::METHOD, "port::File::Code", Some("port::File")),
                (node_kind::STRUCT, "port::Fault", None),
                (node_kind::METHOD, "port::Fault::Code", Some("port::Fault")),
                (node_kind::METHOD, "port::Fault::Error", Some("port::Fault")),
            ],
        );
        file.refs = vec![
            embed(
                "port",
                "port::ReadCloser",
                CallQualifier::Attribute { base: "io".to_string(), name: "Reader".to_string() },
            ),
            embed("port", "port::CodedError", CallQualifier::Bare("error".to_string())),
        ];
        let (read_closer, coded, file_ty, fault) = (
            gid(node_kind::INTERFACE, "port::ReadCloser"),
            gid(node_kind::INTERFACE, "port::CodedError"),
            gid(node_kind::STRUCT, "port::File"),
            gid(node_kind::STRUCT, "port::Fault"),
        );
        let (g, stats) = go_implicit(vec![file]);
        assert_eq!(implements_edge(&g, file_ty, read_closer), None);
        assert_eq!(implements_edge(&g, file_ty, coded), None, "File has Code but no Error");
        assert_eq!(implements_edge(&g, fault, coded), Some(Confidence::Medium));
        assert_eq!(
            stats.map(|s| s.marker()).as_deref(),
            Some("[iface] go implicit implements: 1 (interfaces=1 types=1 embedded=0 open=1)")
        );
    }

    /// An unexported method name belongs to its package: a type in another
    /// directory with the same `isSealed` does not implement the interface.
    #[test]
    fn go_unexported_method_matches_only_its_package() {
        let parses = vec![
            go_file(
                "a::sealed",
                &[
                    (node_kind::INTERFACE, "a::sealed::Sealed", None),
                    (node_kind::METHOD, "a::sealed::Sealed::isSealed", Some("a::sealed::Sealed")),
                ],
            ),
            go_file(
                "a::impl",
                &[
                    (node_kind::STRUCT, "a::impl::T", None),
                    (node_kind::METHOD, "a::impl::T::isSealed", Some("a::impl::T")),
                ],
            ),
            go_file(
                "b::impl",
                &[
                    (node_kind::STRUCT, "b::impl::U", None),
                    (node_kind::METHOD, "b::impl::U::isSealed", Some("b::impl::U")),
                ],
            ),
        ];
        let sealed = gid(node_kind::INTERFACE, "a::sealed::Sealed");
        let (g, _) = go_implicit(parses);
        let same_pkg = gid(node_kind::STRUCT, "a::impl::T");
        assert_eq!(implements_edge(&g, same_pkg, sealed), Some(Confidence::Medium));
        assert_eq!(implements_edge(&g, gid(node_kind::STRUCT, "b::impl::U"), sealed), None);
    }

    /// Embeds bind to an INTERFACE only, package-scoped: `Reader` is the
    /// same-directory interface in another file (not a same-named DATA_ENTITY
    /// of the embedding file, not an interface of another package); `store.
    /// Writer` is the interface of the imported package, also when the go.mod
    /// sits below the repo root (`backend::internal::store`); `io.Closer` and
    /// an ambiguous name stay unresolved.
    #[test]
    fn go_embed_binds_package_scoped_interfaces_only() {
        let mut api = go_file(
            "backend::svc::api",
            &[
                (node_kind::INTERFACE, "backend::svc::api::Port", None),
                (node_kind::DATA_ENTITY, "backend::svc::api::Reader", None),
            ],
        );
        api.imports = vec![
            ImportStmt {
                from_module: "backend::svc::api".to_string(),
                target: ImportTarget::Module { path: "internal::store".to_string(), alias: None },
            },
            ImportStmt {
                from_module: "backend::svc::api".to_string(),
                target: ImportTarget::Module { path: "io".to_string(), alias: None },
            },
        ];
        let port = "backend::svc::api::Port";
        api.refs = vec![
            embed("backend::svc::api", port, CallQualifier::Bare("Reader".to_string())),
            embed(
                "backend::svc::api",
                port,
                CallQualifier::Attribute { base: "store".to_string(), name: "Writer".to_string() },
            ),
            embed(
                "backend::svc::api",
                port,
                CallQualifier::Attribute { base: "io".to_string(), name: "Closer".to_string() },
            ),
            embed("backend::svc::api", port, CallQualifier::Bare("Dup".to_string())),
        ];
        let parses = vec![
            api,
            go_file("backend::svc::read", &[(node_kind::INTERFACE, "backend::svc::read::Reader", None)]),
            go_file("backend::other::read", &[(node_kind::INTERFACE, "backend::other::read::Reader", None)]),
            go_file(
                "backend::internal::store::write",
                &[(node_kind::INTERFACE, "backend::internal::store::write::Writer", None)],
            ),
            go_file("backend::svc::dup_a", &[(node_kind::INTERFACE, "backend::svc::dup_a::Dup", None)]),
            go_file("backend::svc::dup_b", &[(node_kind::INTERFACE, "backend::svc::dup_b::Dup", None)]),
        ];
        let (g, _) = go_implicit(parses);
        let from = gid(node_kind::INTERFACE, port);
        let mut embedded: Vec<NodeId> = g
            .edges
            .iter()
            .filter(|e| e.from == from && e.category == edge_category::INHERITS_FROM)
            .map(|e| e.to)
            .collect();
        embedded.sort_unstable_by_key(|id| id.0);
        let mut expect = vec![
            gid(node_kind::INTERFACE, "backend::svc::read::Reader"),
            gid(node_kind::INTERFACE, "backend::internal::store::write::Writer"),
        ];
        expect.sort_unstable_by_key(|id| id.0);
        assert_eq!(embedded, expect);
        let unbound: Vec<&CallQualifier> =
            g.unresolved_refs.iter().filter(|r| r.from == from).map(|r| &r.qualifier).collect();
        assert_eq!(
            unbound,
            vec![
                &CallQualifier::Attribute { base: "io".to_string(), name: "Closer".to_string() },
                &CallQualifier::Bare("Dup".to_string()),
            ]
        );
    }

    /// An empty interface is implemented by nothing, and a graph with no
    /// INTERFACE reports no marker.
    #[test]
    fn go_empty_interface_and_no_interface() {
        let file = go_file(
            "any",
            &[
                (node_kind::INTERFACE, "any::Anything", None),
                (node_kind::STRUCT, "any::T", None),
                (node_kind::METHOD, "any::T::Run", Some("any::T")),
            ],
        );
        let (g, stats) = go_implicit(vec![file]);
        assert!(!g.edges.iter().any(|e| e.category == edge_category::IMPLEMENTS));
        assert_eq!(
            stats.map(|s| s.marker()).as_deref(),
            Some("[iface] go implicit implements: 0 (interfaces=0 types=0 embedded=0 open=0)")
        );
        let (_, none) = go_implicit(vec![go_file("x", &[(node_kind::STRUCT, "x::T", None)])]);
        assert_eq!(none, None);
    }

    // ---- LA.13b: Go package = directory -------------------------------------

    /// The fixtures/go-package-multifile-calls shape: `cmd/main.go` imports
    /// `internal/store` and calls `store.Save()` / `store.Load()`; `Save`
    /// (store.go) calls `helper()` (load.go).
    fn multifile_package_shape() -> Vec<FileParse> {
        let mut main = go_file("cmd::main", &[(node_kind::FUNCTION, "cmd::main::main", None)]);
        main.imports = vec![ImportStmt {
            from_module: "cmd::main".to_string(),
            target: ImportTarget::Module { path: "internal::store".to_string(), alias: None },
        }];
        let from = gid(node_kind::FUNCTION, "cmd::main::main");
        let attr = |name: &str| CallQualifier::Attribute { base: "store".to_string(), name: name.to_string() };
        main.calls = vec![
            CallSite { from, qualifier: attr("Save") },
            CallSite { from, qualifier: attr("Load") },
        ];
        let mut store =
            go_file("internal::store::store", &[(node_kind::FUNCTION, "internal::store::store::Save", None)]);
        store.calls = vec![CallSite {
            from: gid(node_kind::FUNCTION, "internal::store::store::Save"),
            qualifier: CallQualifier::Bare("helper".to_string()),
        }];
        let load = go_file(
            "internal::store::load",
            &[
                (node_kind::FUNCTION, "internal::store::load::Load", None),
                (node_kind::FUNCTION, "internal::store::load::helper", None),
            ],
        );
        vec![main, store, load]
    }

    /// The `[go-package]` marker counts the package directories, the
    /// multi-file ones, the imports the directory step bound and the calls
    /// bound in a sibling file; the fixture prints the packet's marker.
    #[test]
    fn go_package_stats_and_marker() {
        let (g, _, _, stats) = build_go_passes(repo(), multifile_package_shape());
        assert_eq!(
            stats,
            GoPackageStats { dirs: 2, multi_file: 1, dir_bound_imports: 1, sibling_calls: 2 }
        );
        assert_eq!(
            stats.marker().as_deref(),
            Some("[go-package] dirs=2 multi_file=1 dir_bound_imports=1 sibling_calls=2")
        );
        let main = gid(node_kind::FUNCTION, "cmd::main::main");
        let save = gid(node_kind::FUNCTION, "internal::store::store::Save");
        assert!(has_edge(&g, main, save, edge_category::CALLS));
        assert!(has_edge(&g, main, gid(node_kind::FUNCTION, "internal::store::load::Load"), edge_category::CALLS));
        assert!(has_edge(&g, save, gid(node_kind::FUNCTION, "internal::store::load::helper"), edge_category::CALLS));
        assert!(g.unresolved_calls.is_empty());

        let (_, _, _, empty) = build_go_passes(repo(), vec![]);
        assert_eq!(empty.marker(), None, "no MODULE, no marker");
    }

    /// An import of a directory binds its dir-named file, else its first
    /// non-test file by qname, else its first test file, never the importer,
    /// whatever order the files arrive in; a root-level file's package is
    /// `""`.
    #[test]
    fn go_import_target_prefers_dir_named_then_non_test_files() {
        let m = |q: &str| gid(node_kind::MODULE, q);
        for order in [[0, 1, 2, 3, 4, 5, 6], [6, 5, 4, 3, 2, 1, 0]] {
            let files = [
                go_file("a::store::zeta", &[]),
                go_file("a::store::store", &[]),
                go_file("b::util::y", &[]),
                go_file("b::util::a_test", &[]),
                go_file("c::only::only_test", &[]),
                go_file("c::only::more_test", &[]),
                go_file("main", &[]),
            ];
            let parses: Vec<FileParse> = order.iter().map(|&i| files[i].clone()).collect();
            let (mut g, imports, _, _) = merge_parses(repo(), parses);
            build_symbol_table(&mut g);
            let pk = GoPackages::build(&g, &imports);
            let importer = m("main");
            assert_eq!(pk.import_target("a::store", importer), DirImport::Bound(m("a::store::store")));
            assert_eq!(pk.import_target("b::util", importer), DirImport::Bound(m("b::util::y")));
            assert_eq!(pk.import_target("c::only", importer), DirImport::Bound(m("c::only::more_test")));
            assert_eq!(
                pk.import_target("c::only", m("c::only::more_test")),
                DirImport::Bound(m("c::only::only_test"))
            );
            assert_eq!(pk.import_target("a", importer), DirImport::NoDir);
            assert_eq!(pk.import_target("", m("a::store::zeta")), DirImport::Bound(importer));
            assert_eq!(pk.import_target("", importer), DirImport::OnlyImporter);
        }
    }
}
