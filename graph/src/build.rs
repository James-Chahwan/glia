//! Per-language graph builders plus the shared merge / nav / symbol-table
//! passes they all run.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, ImportStmt, UnresolvedRef, edge_category,
    node_kind,
};
use repo_graph_core::{Cell, NodeId, RepoId};

use crate::calls::{emit_method_level_implements, push_edge, resolve_calls, resolve_refs};
use crate::imports::{
    resolve_imports_go, resolve_imports_python, resolve_imports_slash, resolve_imports_ts,
};
use crate::rust_paths::{RustCrate, RustIndex};
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
pub fn build_go(repo: RepoId, parses: Vec<FileParse>) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    let split = bind_split_go_receivers(&mut g);
    build_symbol_table(&mut g);
    resolve_imports_go(&mut g, &all_imports);
    resolve_go_calls(&mut g, &all_calls, &split);
    resolve_refs(&mut g, &all_refs);
    emit_method_level_implements(&mut g);
    if let Some(line) = split.marker() {
        eprintln!("{line}");
    }
    Ok(g)
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
pub fn build_rust(
    repo: RepoId,
    parses: Vec<FileParse>,
    crates: &[RustCrate],
) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    resolve_imports_python(&mut g, &all_imports);
    let idx = RustIndex::build(&g, crates);
    // Inline-mod pre-pass: a hit is a CALLS edge now; a miss (and every other
    // site) keeps its original position for the generic pass.
    let mut rest: Vec<CallSite> = Vec::with_capacity(all_calls.len());
    let mut mod_scoped = 0usize;
    for site in all_calls {
        match idx.resolve_scoped_bare(&g, &site) {
            Some(to) => {
                push_edge(&mut g, site.from, to, edge_category::CALLS);
                mod_scoped += 1;
            }
            None => rest.push(site),
        }
    }
    resolve_calls(&mut g, &rest, |g, site| idx.resolve_call(g, site));
    resolve_refs(&mut g, &all_refs);
    idx.resolve_leftover_refs(&mut g);
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
// stack on a single Module node (Go packages, TS re-exports, etc.).
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
    fn package_dir(module_qname: &str) -> &str {
        module_qname.rsplit_once("::").map_or("", |(dir, _)| dir)
    }
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
                    let ids = types.entry((package_dir(module_qname), name.as_str())).or_default();
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
                candidates.push((n.id, module, package_dir(module_qname), recv));
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
fn resolve_go_calls(g: &mut RepoGraph, calls: &[CallSite], split: &SplitStats) {
    if split.bound.is_empty() {
        resolve_calls(g, calls, |_, _| None);
        return;
    }
    let bound: HashSet<NodeId> = split.bound.iter().map(|&(m, _, _)| m).collect();
    let (file_scoped, rest): (Vec<CallSite>, Vec<CallSite>) =
        calls.iter().cloned().partition(|s| {
            matches!(s.qualifier, CallQualifier::Bare(_) | CallQualifier::Attribute { .. })
                && under_bound_method(&g.nav, &bound, s.from)
        });
    resolve_calls(g, &rest, |_, _| None);
    for &(method, module, _) in &split.bound {
        g.nav.parent_of.insert(method, module);
    }
    resolve_calls(g, &file_scoped, |_, _| None);
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
}
