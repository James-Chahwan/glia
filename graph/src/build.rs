//! Per-language graph builders plus the shared merge / nav / symbol-table
//! passes they all run.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{CallSite, CodeNav, FileParse, ImportStmt, UnresolvedRef, node_kind};
use repo_graph_core::{Cell, NodeId, RepoId};

use crate::calls::{emit_method_level_implements, resolve_calls, resolve_refs};
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

/// Build a per-repo Go graph. Go packages span multiple files — modules with
/// the same qname produce the same NodeId and their cells stack on one node.
pub fn build_go(repo: RepoId, parses: Vec<FileParse>) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    resolve_imports_go(&mut g, &all_imports);
    resolve_calls(&mut g, &all_calls, |_, _| None);
    resolve_refs(&mut g, &all_refs);
    emit_method_level_implements(&mut g);
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
pub fn build_rust(
    repo: RepoId,
    parses: Vec<FileParse>,
    crates: &[RustCrate],
) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    resolve_imports_python(&mut g, &all_imports);
    let idx = RustIndex::build(&g, crates);
    resolve_calls(&mut g, &all_calls, |g, site| idx.resolve_call(g, site));
    resolve_refs(&mut g, &all_refs);
    emit_method_level_implements(&mut g);
    idx.report();
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
}
