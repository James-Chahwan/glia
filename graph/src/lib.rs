//! repo-graph-graph — per-repo graph construction + resolver + traversal.
//!
//! Consumes `FileParse` outputs from the language parsers, merges them into a
//! single `RepoGraph` for the repo, resolves cross-file imports and calls
//! using a symbol table, and exposes BFS / neighbours / parent-chain.
//!
//! One entry point per parser — `build_python`, `build_go`, `build_typescript`
//! — because their import semantics differ (dotted qnames vs. stripped go.mod
//! paths vs. relative/bare module sources). The symbol-table + traversal
//! infrastructure is shared.
//!
//! v0.4.4b adds `MergedGraph` + `CrossGraphResolver` for cross-repo resolution.
//! The first resolver, `HttpStackResolver`, pairs frontend Endpoints with
//! backend Routes by (method, normalised path) and emits `HTTP_CALLS` edges.
//! Other stack resolvers (GraphQL, gRPC, queues, shared-schema) land at v0.4.10
//! against the same trait.

use std::collections::{HashMap, HashSet, VecDeque};

use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, ImportStmt, ImportTarget, UnresolvedRef,
    cell_type, edge_category, node_kind,
};
use repo_graph_core::{
    Cell, CellPayload, Confidence, Edge, EdgeCategoryId, Node, NodeId, NodeKindId, RepoId,
};

// ============================================================================
// Output graph
// ============================================================================

#[derive(Debug)]
pub struct RepoGraph {
    pub repo: RepoId,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    pub symbols: SymbolTable,
    /// Call sites left unresolved after cross-file resolution. Kept as a
    /// diagnostic surface (at v0.4.5 they also feed the dense-text `?` sigil).
    pub unresolved_calls: Vec<CallSite>,
    /// `UnresolvedRef`s the resolver couldn't bind. Same diagnostic role as
    /// `unresolved_calls`. v0.4.4 use case: gin route handler refs that point
    /// at packages the parser couldn't link to a known module.
    pub unresolved_refs: Vec<UnresolvedRef>,
    /// v0.4.13b — method ids tagged as property-style (read as `self.x`).
    /// Populated from `FileParse.properties`. Consumed by composition-path
    /// synthesis to filter method→class hops down to syntactically valid
    /// attribute reads.
    pub properties: HashSet<NodeId>,
}

/// Symbol index built during resolution. Everything keyed by node id so
/// consumers never re-parse qnames.
#[derive(Debug, Default)]
pub struct SymbolTable {
    /// Module qname (`"myapp::users"`) → module node id.
    pub module_by_qname: HashMap<String, NodeId>,
    /// Module node id → (top-level def name → def node id).
    /// Used for `from X import Y` resolution and for module-attribute calls.
    pub module_symbols: HashMap<NodeId, HashMap<String, NodeId>>,
    /// Class node id → (method name → method node id).
    pub class_methods: HashMap<NodeId, HashMap<String, NodeId>>,
    /// Module node id → (bound name in that module → target node id).
    /// Populated from resolved imports. Powers cross-file call resolution.
    pub module_import_bindings: HashMap<NodeId, HashMap<String, NodeId>>,
}

// ============================================================================
// Errors
// ============================================================================

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("module qname collision: {0}")]
    ModuleCollision(String),
}

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
    Ok(g)
}

/// Build a per-repo graph for languages whose import paths are dotted
/// (`foo.bar.Baz`) or already normalised to `::` form. Reuses the Python
/// resolver because `.replace('.', "::")` is a no-op on already-`::` paths.
/// Covers Java, C#, PHP, Rust, Scala, Clojure, Elixir.
pub fn build_dotted(repo: RepoId, parses: Vec<FileParse>) -> Result<RepoGraph, GraphError> {
    let (mut g, all_imports, all_calls, all_refs) = merge_parses(repo, parses);
    build_symbol_table(&mut g);
    resolve_imports_python(&mut g, &all_imports);
    resolve_calls(&mut g, &all_calls, |_, _| None);
    resolve_refs(&mut g, &all_refs);
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
        if parent_kind == Some(node_kind::MODULE) {
            let entry = g.symbols.module_symbols.entry(*parent).or_default();
            for child in children {
                if let Some(name) = g.nav.name_by_id.get(child) {
                    entry.insert(name.clone(), *child);
                }
            }
        } else if parent_kind == Some(node_kind::CLASS) || parent_kind == Some(node_kind::STRUCT) {
            let entry = g.symbols.class_methods.entry(*parent).or_default();
            for child in children {
                if let Some(name) = g.nav.name_by_id.get(child)
                    && g.nav.kind_by_id.get(child) == Some(&node_kind::METHOD)
                {
                    entry.insert(name.clone(), *child);
                }
            }
        }
    }
}

// ============================================================================
// Import resolution
// ============================================================================

fn resolve_imports_python(g: &mut RepoGraph, imports: &[ImportStmt]) {
    for stmt in imports {
        let Some(from_mod_id) = g
            .symbols
            .module_by_qname
            .get(&stmt.from_module)
            .copied()
        else {
            continue;
        };

        match &stmt.target {
            ImportTarget::Module { path, alias } => {
                // `import foo.bar` — convert `.` → `::` and look up by qname.
                let target_qname = path.replace('.', "::");
                if let Some(target_id) = g.symbols.module_by_qname.get(&target_qname).copied() {
                    push_edge(g, from_mod_id, target_id, edge_category::IMPORTS);
                    let bound_name = alias.clone().unwrap_or_else(|| {
                        path.split('.').next().unwrap_or(path).to_string()
                    });
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(bound_name, target_id);
                }
            }
            ImportTarget::Symbol { module, name, alias, level } => {
                let target_module_qname = resolve_module_reference(&stmt.from_module, module, *level);

                // Try `module::name` as a submodule first — matches Python's
                // `from pkg import mod` → edge to pkg.mod if submodule exists.
                let submodule_qname = if target_module_qname.is_empty() {
                    name.clone()
                } else {
                    format!("{target_module_qname}::{name}")
                };

                let bound = alias.clone().unwrap_or_else(|| name.clone());

                if let Some(submodule_id) = g.symbols.module_by_qname.get(&submodule_qname).copied()
                {
                    // `from pkg import mod` where mod is a submodule.
                    push_edge(g, from_mod_id, submodule_id, edge_category::IMPORTS);
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(bound, submodule_id);
                } else if let Some(target_mod_id) = g
                    .symbols
                    .module_by_qname
                    .get(&target_module_qname)
                    .copied()
                {
                    // `from pkg.mod import Name` — target is a symbol inside pkg.mod.
                    push_edge(g, from_mod_id, target_mod_id, edge_category::IMPORTS);
                    if let Some(symbol_id) = g
                        .symbols
                        .module_symbols
                        .get(&target_mod_id)
                        .and_then(|t| t.get(name))
                        .copied()
                    {
                        g.symbols
                            .module_import_bindings
                            .entry(from_mod_id)
                            .or_default()
                            .insert(bound, symbol_id);
                    }
                }
            }
        }
    }
}

/// Go imports: the parser has already stripped the go.mod prefix and produced
/// `ImportTarget::Module { path }` with `path` = repo-local `::` qname for
/// imports that resolve inside this module. External imports keep the raw
/// `std::io`-style form and won't match anything.
fn resolve_imports_go(g: &mut RepoGraph, imports: &[ImportStmt]) {
    for stmt in imports {
        let Some(from_mod_id) = g
            .symbols
            .module_by_qname
            .get(&stmt.from_module)
            .copied()
        else {
            continue;
        };
        let ImportTarget::Module { path, alias } = &stmt.target else {
            continue;
        };
        let Some(target_id) = g.symbols.module_by_qname.get(path).copied() else {
            continue;
        };
        push_edge(g, from_mod_id, target_id, edge_category::IMPORTS);
        let bound = alias
            .clone()
            .unwrap_or_else(|| path.rsplit("::").next().unwrap_or(path).to_string());
        g.symbols
            .module_import_bindings
            .entry(from_mod_id)
            .or_default()
            .insert(bound, target_id);
    }
}

/// Ruby imports: `require 'foo/bar'` gives a slash-delimited path. Convert
/// slashes to `::` then look up directly (same shape as Go's resolver).
fn resolve_imports_slash(g: &mut RepoGraph, imports: &[ImportStmt]) {
    for stmt in imports {
        let Some(from_mod_id) = g
            .symbols
            .module_by_qname
            .get(&stmt.from_module)
            .copied()
        else {
            continue;
        };
        let ImportTarget::Module { path, alias } = &stmt.target else {
            continue;
        };
        let target_qname = path.replace('/', "::");
        let Some(target_id) = g.symbols.module_by_qname.get(&target_qname).copied() else {
            continue;
        };
        push_edge(g, from_mod_id, target_id, edge_category::IMPORTS);
        let bound = alias
            .clone()
            .unwrap_or_else(|| target_qname.rsplit("::").next().unwrap_or(&target_qname).to_string());
        g.symbols
            .module_import_bindings
            .entry(from_mod_id)
            .or_default()
            .insert(bound, target_id);
    }
}

/// TypeScript imports: the parser keeps import sources as raw strings
/// (`./user`, `@angular/core`). `resolve_source(from_qname, raw)` converts a
/// raw source string to a module qname; `None` marks the import external.
fn resolve_imports_ts<R: Fn(&str, &str) -> Option<String>>(
    g: &mut RepoGraph,
    imports: &[ImportStmt],
    resolve_source: &R,
) {
    for stmt in imports {
        let Some(from_mod_id) = g
            .symbols
            .module_by_qname
            .get(&stmt.from_module)
            .copied()
        else {
            continue;
        };
        match &stmt.target {
            ImportTarget::Module { path, alias } => {
                let Some(target_qname) = resolve_source(&stmt.from_module, path) else {
                    continue;
                };
                let Some(target_id) = g.symbols.module_by_qname.get(&target_qname).copied() else {
                    continue;
                };
                push_edge(g, from_mod_id, target_id, edge_category::IMPORTS);
                // Namespace import alias is the binding; bare side-effect has none.
                if let Some(a) = alias {
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(a.clone(), target_id);
                }
            }
            ImportTarget::Symbol {
                module,
                name,
                alias,
                ..
            } => {
                let Some(target_qname) = resolve_source(&stmt.from_module, module) else {
                    continue;
                };
                let Some(target_mod_id) = g.symbols.module_by_qname.get(&target_qname).copied()
                else {
                    continue;
                };
                push_edge(g, from_mod_id, target_mod_id, edge_category::IMPORTS);
                let bound = alias.clone().unwrap_or_else(|| name.clone());
                // Default import — bind to the module itself.
                // Named import — bind to the specific symbol inside that module.
                let target_id = if name == "default" {
                    Some(target_mod_id)
                } else {
                    g.symbols
                        .module_symbols
                        .get(&target_mod_id)
                        .and_then(|s| s.get(name))
                        .copied()
                };
                if let Some(t) = target_id {
                    g.symbols
                        .module_import_bindings
                        .entry(from_mod_id)
                        .or_default()
                        .insert(bound, t);
                }
            }
        }
    }
}

/// Convert a (possibly relative) `from X import Y` module reference into an
/// absolute qname using `::` separators.
fn resolve_module_reference(from_module: &str, module_ref: &str, level: u32) -> String {
    if level == 0 {
        return module_ref.replace('.', "::");
    }
    // Relative: strip `level` trailing components from `from_module`, then
    // append `module_ref`. `level=1` pops 1 (the current file stays at package level).
    let mut parts: Vec<&str> = from_module.split("::").collect();
    for _ in 0..level {
        parts.pop();
    }
    let base = parts.join("::");
    if module_ref.is_empty() {
        base
    } else if base.is_empty() {
        module_ref.replace('.', "::")
    } else {
        format!("{base}::{}", module_ref.replace('.', "::"))
    }
}

// ============================================================================
// Call resolution
// ============================================================================

/// Cross-file call resolution — same recipe for all languages.
///
/// `extra_hook` is an escape hatch for language-specific resolution shapes
/// that the generic pass doesn't cover. Unused today (pass `|_, _| None`);
/// it's the seam for future Go method-on-struct-via-package-alias lookups
/// and similar language-specific call shapes.
fn resolve_calls<H>(g: &mut RepoGraph, calls: &[CallSite], extra_hook: H)
where
    H: Fn(&RepoGraph, &CallSite) -> Option<NodeId>,
{
    for site in calls {
        let Some(from_module) = enclosing_module(&g.nav, site.from) else {
            g.unresolved_calls.push(site.clone());
            continue;
        };
        let bindings = g.symbols.module_import_bindings.get(&from_module);

        let resolved: Option<NodeId> = match &site.qualifier {
            CallQualifier::Bare(name) => {
                // Priority: local import binding → same-module top-level def.
                bindings
                    .and_then(|b| b.get(name).copied())
                    .or_else(|| {
                        g.symbols
                            .module_symbols
                            .get(&from_module)
                            .and_then(|s| s.get(name).copied())
                    })
            }
            CallQualifier::Attribute { base, name } => bindings
                .and_then(|b| b.get(base).copied())
                .and_then(|base_id| {
                    let base_kind = g.nav.kind_by_id.get(&base_id).copied();
                    if base_kind == Some(node_kind::MODULE) {
                        g.symbols
                            .module_symbols
                            .get(&base_id)
                            .and_then(|s| s.get(name).copied())
                    } else if base_kind == Some(node_kind::CLASS)
                        || base_kind == Some(node_kind::STRUCT)
                    {
                        g.symbols
                            .class_methods
                            .get(&base_id)
                            .and_then(|m| m.get(name).copied())
                    } else {
                        None
                    }
                }),
            CallQualifier::SelfMethod(name) => {
                enclosing_class_or_struct(&g.nav, site.from).and_then(|parent_id| {
                    g.symbols
                        .class_methods
                        .get(&parent_id)
                        .and_then(|m| m.get(name).copied())
                })
            }
            // Python `super().m()` — intra-file super calls are resolved by
            // the Python parser before emitting the CallSite. Anything that
            // reaches this layer is cross-file (base class imported from
            // another module) and requires walking the enclosing class's
            // recorded base-class names through `module_import_bindings`.
            // Not wired at v0.4.13 — falls through to extra_hook / unresolved.
            CallQualifier::SuperMethod(_) => None,
            CallQualifier::ComplexReceiver { .. } => None,
        };

        let resolved = resolved.or_else(|| extra_hook(g, site));

        match resolved {
            Some(to) => push_edge(g, site.from, to, edge_category::CALLS),
            None => g.unresolved_calls.push(site.clone()),
        }
    }
}

/// Resolve `UnresolvedRef`s the same way `resolve_calls` resolves `CallSite`s,
/// but using the ref's `from_module` directly (refs come from sources like
/// Route nodes that have no enclosing module to walk to) and emitting an edge
/// of the ref's declared `category` instead of CALLS.
///
/// Today's only producer is parser-go's route extraction, where `category` is
/// `HANDLED_BY` and the qualifier shape is either `Bare(name)` (handler is a
/// same-package fn) or `Attribute { base, name }` (handler is `pkg.Name`).
fn resolve_refs(g: &mut RepoGraph, refs: &[UnresolvedRef]) {
    for r in refs {
        let bindings = g.symbols.module_import_bindings.get(&r.from_module);
        let resolved: Option<NodeId> = match &r.qualifier {
            CallQualifier::Bare(name) => bindings
                .and_then(|b| b.get(name).copied())
                .or_else(|| {
                    g.symbols
                        .module_symbols
                        .get(&r.from_module)
                        .and_then(|s| s.get(name).copied())
                })
                // Global fallback for HANDLED_BY refs: a route registers
                // `r.GET("/p", handler)` where `handler` is a top-level fn
                // in the same package — but `bindings` doesn't see local
                // package symbols. Scan all module_symbols for a unique
                // match. Same-name collisions across the repo skip
                // (better unresolved than wrong).
                .or_else(|| {
                    if r.category == edge_category::HANDLED_BY {
                        unique_global_function(g, name)
                    } else {
                        None
                    }
                }),
            CallQualifier::Attribute { base, name } => bindings
                .and_then(|b| b.get(base).copied())
                .and_then(|base_id| {
                    let base_kind = g.nav.kind_by_id.get(&base_id).copied();
                    if base_kind == Some(node_kind::MODULE) {
                        g.symbols
                            .module_symbols
                            .get(&base_id)
                            .and_then(|s| s.get(name).copied())
                    } else if base_kind == Some(node_kind::CLASS)
                        || base_kind == Some(node_kind::STRUCT)
                    {
                        g.symbols
                            .class_methods
                            .get(&base_id)
                            .and_then(|m| m.get(name).copied())
                    } else {
                        None
                    }
                })
                // Global fallback for HANDLED_BY: in Go, route handlers
                // are usually written `h.GetProfile` where `h` is a local
                // struct-receiver variable (`h *Handlers`), not an import
                // binding. So binding lookup fails. Scan all class_methods
                // across the graph for a method matching `name`; emit
                // only when exactly one match exists.
                .or_else(|| {
                    if r.category == edge_category::HANDLED_BY {
                        unique_global_method(g, name)
                    } else {
                        None
                    }
                }),
            CallQualifier::SelfMethod(_)
            | CallQualifier::SuperMethod(_)
            | CallQualifier::ComplexReceiver { .. } => None,
        };

        match resolved {
            Some(to) => push_edge(g, r.from, to, r.category),
            None => g.unresolved_refs.push(r.clone()),
        }
    }
}

/// Search every class/struct's method map for a method named `name`.
/// Returns the NodeId iff exactly one class has it (avoids fabricating
/// edges when the same method name lives on multiple types).
fn unique_global_method(g: &RepoGraph, name: &str) -> Option<NodeId> {
    let mut hit: Option<NodeId> = None;
    for methods in g.symbols.class_methods.values() {
        if let Some(&id) = methods.get(name) {
            if hit.is_some() {
                return None; // ambiguous
            }
            hit = Some(id);
        }
    }
    hit
}

/// Same idea for top-level functions across the repo.
fn unique_global_function(g: &RepoGraph, name: &str) -> Option<NodeId> {
    let mut hit: Option<NodeId> = None;
    for syms in g.symbols.module_symbols.values() {
        if let Some(&id) = syms.get(name) {
            if hit.is_some() {
                return None; // ambiguous
            }
            hit = Some(id);
        }
    }
    hit
}

/// Walk `parent_of` until we hit a module node. For a top-level function this
/// returns its module directly; for a method it walks method → class → module.
fn enclosing_module(nav: &CodeNav, mut id: NodeId) -> Option<NodeId> {
    loop {
        if nav.kind_by_id.get(&id) == Some(&node_kind::MODULE) {
            return Some(id);
        }
        id = *nav.parent_of.get(&id)?;
    }
}

/// Walk parents to find the enclosing CLASS or STRUCT. Used to resolve
/// self-method calls (Go `u.Save()`, TS `this.save()`, etc.) to a sibling
/// method on the same type.
fn enclosing_class_or_struct(nav: &CodeNav, start: NodeId) -> Option<NodeId> {
    let mut cur = start;
    loop {
        let parent = *nav.parent_of.get(&cur)?;
        let k = nav.kind_by_id.get(&parent).copied();
        if k == Some(node_kind::CLASS) || k == Some(node_kind::STRUCT) {
            return Some(parent);
        }
        cur = parent;
    }
}

fn push_edge(g: &mut RepoGraph, from: NodeId, to: NodeId, category: EdgeCategoryId) {
    g.edges.push(Edge {
        from,
        to,
        category,
        confidence: Confidence::Strong,
    });
}

// ============================================================================
// Cross-graph resolution (v0.4.4b)
// ============================================================================

/// A bundle of per-repo `RepoGraph`s plus edges that cross repo boundaries.
///
/// Per-repo graphs stay owned and addressable by their `RepoId`. Cross-edges
/// sit on the merged container so the per-repo graphs remain round-trippable
/// through the v0.4.5 rkyv store without the intra-repo edge list being
/// polluted by cross-repo references that only make sense once multiple repos
/// are in scope.
#[derive(Debug, Default)]
pub struct MergedGraph {
    pub graphs: Vec<RepoGraph>,
    pub cross_edges: Vec<Edge>,
}

impl MergedGraph {
    pub fn new(graphs: Vec<RepoGraph>) -> Self {
        Self {
            graphs,
            cross_edges: Vec::new(),
        }
    }

    pub fn run<R: CrossGraphResolver>(&mut self, resolver: &R) {
        resolver.resolve(self);
    }

    /// All cross-repo edges plus each per-repo graph's intra edges. Used by
    /// consumers that want a single iterator over the whole merged graph.
    pub fn all_edges(&self) -> impl Iterator<Item = &Edge> + '_ {
        self.graphs
            .iter()
            .flat_map(|g| g.edges.iter())
            .chain(self.cross_edges.iter())
    }

    /// G20 — visit every node of `kind` across every contained repo. Iteration
    /// order is per-repo, in repo insertion order. The callback receives the
    /// `(NodeId, &Node)` pair so consumers can read cells / confidence without
    /// a second lookup.
    pub fn for_each_node_of_kind<F: FnMut(NodeId, &Node)>(&self, kind: NodeKindId, mut f: F) {
        for g in &self.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id).copied() == Some(kind) {
                    f(n.id, n);
                }
            }
        }
    }

    /// G20 — collect every node of `kind` into a `Vec<NodeId>`. Equivalent to
    /// `for_each_node_of_kind` but materialised; convenient for callers that
    /// need to hold the list while doing other graph work.
    pub fn nodes_of_kind(&self, kind: NodeKindId) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.for_each_node_of_kind(kind, |id, _| out.push(id));
        out
    }

    /// G20 — build a pre-computed kind → `Vec<NodeId>` index spanning every
    /// repo in the merged graph. Built lazily by callers that want to do many
    /// kind-scoped walks (e.g. neuropil's OpenAPI catalog touches both
    /// `ROUTE` and `ENDPOINT`); cache the return on the caller side.
    pub fn kind_index(&self) -> std::collections::HashMap<NodeKindId, Vec<NodeId>> {
        let mut out: std::collections::HashMap<NodeKindId, Vec<NodeId>> =
            std::collections::HashMap::new();
        for g in &self.graphs {
            for n in &g.nodes {
                if let Some(&kind) = g.nav.kind_by_id.get(&n.id) {
                    out.entry(kind).or_default().push(n.id);
                }
            }
        }
        out
    }

    /// Total degree (incoming + outgoing) of `id` across both intra- and
    /// cross-repo edges. Used purely as the determinism tiebreak in
    /// [`Self::pick_primary`].
    fn degree(&self, id: NodeId) -> usize {
        self.all_edges()
            .filter(|e| e.from == id || e.to == id)
            .count()
    }

    /// Deterministically choose the "primary" node among identically-keyed
    /// candidates (nodes sharing a simple name or a qname). Two real cases hit
    /// this: framework parsers stack a `COMPONENT`/route marker on top of the
    /// underlying `CLASS`, sharing name *and* qname; and suffix matches collide
    /// across repos.
    ///
    /// Rule: highest total degree wins — the node that actually participates in
    /// the graph is what traversal, `impact`, and span resolution want, not an
    /// edgeless marker. Ties break on the lowest `NodeId`. Both keys are stable
    /// across processes, so the choice no longer rides on `HashMap` iteration
    /// order — that randomness was the root of the intermittent-empty
    /// `impact`/`trace` results (an Angular `GroupsComponent` resolving to the
    /// edgeless `COMPONENT` marker instead of the 80-downstream `CLASS`).
    pub fn pick_primary(&self, candidates: &[NodeId]) -> Option<NodeId> {
        match candidates {
            [] => None,
            [only] => Some(*only),
            many => many
                .iter()
                .copied()
                .max_by_key(|&id| (self.degree(id), std::cmp::Reverse(id.0))),
        }
    }

    /// Resolve a simple name (`"GroupsComponent"`) to a single `NodeId`,
    /// deterministically. When several nodes share the name, the highest-degree
    /// one wins (see [`Self::pick_primary`]). `None` if no node carries it.
    pub fn resolve_name(&self, name: &str) -> Option<NodeId> {
        let mut matches = Vec::new();
        for g in &self.graphs {
            for (id, n) in &g.nav.name_by_id {
                if n == name {
                    matches.push(*id);
                }
            }
        }
        self.pick_primary(&matches)
    }

    /// G22 — resolve a full qualified name to a single `NodeId` across every
    /// repo. Returns `None` if no node carries this exact qname. NodeIds aren't
    /// stable across rebuilds; qnames are, so this is the canonical re-keying
    /// path for view-state persistence (e.g. `.neuropil/view_state.json`).
    ///
    /// When more than one node shares the qname (a framework marker stacked on
    /// its class), the pick is deterministic — see [`Self::pick_primary`].
    pub fn node_id_by_qname(&self, qname: &str) -> Option<NodeId> {
        self.pick_primary(&self.qnames_exact(qname))
    }

    /// Every node whose qname matches `qname` exactly, sorted by `NodeId` for a
    /// stable, reproducible iteration order. Use when each match matters (e.g.
    /// `impact` walks them all); use [`Self::node_id_by_qname`] for the single
    /// primary node.
    pub fn qnames_exact(&self, qname: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        for g in &self.graphs {
            for (id, qn) in &g.nav.qname_by_id {
                if qn == qname {
                    out.push(*id);
                }
            }
        }
        out.sort_by_key(|id| id.0);
        out
    }

    /// Every node whose qname *contains* `pattern`, sorted by `NodeId`. Backs
    /// the pyo3 `find_nodes_by_qname` substring search; sorting keeps the result
    /// reproducible across processes.
    pub fn qnames_containing(&self, pattern: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        for g in &self.graphs {
            for (id, qn) in &g.nav.qname_by_id {
                if qn.contains(pattern) {
                    out.push(*id);
                }
            }
        }
        out.sort_by_key(|id| id.0);
        out
    }

    /// A new `MergedGraph` containing only `keep` nodes, the edges whose both
    /// endpoints are kept, and matching nav entries — structural glue for scoped
    /// projection (WP-C / GR-3: render the top-K from `activate`, not the whole
    /// graph). `symbols` / unresolved / `properties` are dropped (rendering and
    /// read paths don't need them). Cross-benefit: any consumer can scope a view.
    pub fn subset(&self, keep: &[NodeId]) -> MergedGraph {
        let keep: HashSet<NodeId> = keep.iter().copied().collect();
        let graphs = self
            .graphs
            .iter()
            .map(|g| {
                let nodes: Vec<Node> =
                    g.nodes.iter().filter(|n| keep.contains(&n.id)).cloned().collect();
                let edges: Vec<Edge> = g
                    .edges
                    .iter()
                    .filter(|e| keep.contains(&e.from) && keep.contains(&e.to))
                    .cloned()
                    .collect();
                let mut nav = CodeNav::default();
                for (id, v) in &g.nav.name_by_id {
                    if keep.contains(id) {
                        nav.name_by_id.insert(*id, v.clone());
                    }
                }
                for (id, v) in &g.nav.qname_by_id {
                    if keep.contains(id) {
                        nav.qname_by_id.insert(*id, v.clone());
                    }
                }
                for (id, v) in &g.nav.kind_by_id {
                    if keep.contains(id) {
                        nav.kind_by_id.insert(*id, *v);
                    }
                }
                for (id, p) in &g.nav.parent_of {
                    if keep.contains(id) && keep.contains(p) {
                        nav.parent_of.insert(*id, *p);
                    }
                }
                for (id, kids) in &g.nav.children_of {
                    if keep.contains(id) {
                        let kept: Vec<NodeId> =
                            kids.iter().copied().filter(|k| keep.contains(k)).collect();
                        if !kept.is_empty() {
                            nav.children_of.insert(*id, kept);
                        }
                    }
                }
                RepoGraph {
                    repo: g.repo,
                    nodes,
                    edges,
                    nav,
                    symbols: SymbolTable::default(),
                    unresolved_calls: Vec::new(),
                    unresolved_refs: Vec::new(),
                    properties: HashSet::new(),
                }
            })
            .collect();
        let cross_edges = self
            .cross_edges
            .iter()
            .filter(|e| keep.contains(&e.from) && keep.contains(&e.to))
            .cloned()
            .collect();
        MergedGraph { graphs, cross_edges }
    }

    /// G19 — resolve an OTLP-style dotted span name (`myservice.handlers.users.list_users`)
    /// to a `NodeId`. First tries an exact match against the qname (after
    /// converting `.` to `::`); then a suffix match so spans rooted at a
    /// package the parser doesn't see still bind to the method node.
    /// When several nodes share the qname suffix, the pick is deterministic
    /// (highest-degree, see [`Self::pick_primary`]) — consumers that need a
    /// specific repo should still disambiguate by repo.
    pub fn resolve_span(&self, span_name: &str) -> Option<NodeId> {
        let normalised = span_name.replace('.', "::");
        if let Some(id) = self.node_id_by_qname(&normalised) {
            return Some(id);
        }
        let suffix = format!("::{normalised}");
        let mut matches = Vec::new();
        for g in &self.graphs {
            for (id, qn) in &g.nav.qname_by_id {
                if qn.ends_with(&suffix) {
                    matches.push(*id);
                }
            }
        }
        self.pick_primary(&matches)
    }
}

/// G17 — derive a stable cluster key for `node_id` at the requested depth.
/// Splits the node's qname on `::` and returns the first `min(depth, n-1)`
/// segments rejoined. Capping at `n-1` guarantees the leaf segment never
/// lands in the key — siblings share the cluster, the node itself doesn't
/// form a singleton bucket.
///
/// Returns the empty string if `node_id` is unknown to `merged`.
pub fn cluster_key_for(node_id: NodeId, depth: usize, merged: &MergedGraph) -> String {
    let qname = match find_qname(node_id, merged) {
        Some(q) => q,
        None => return String::new(),
    };
    let parts: Vec<&str> = qname.split("::").collect();
    if parts.is_empty() {
        return String::new();
    }
    let take = depth.min(parts.len().saturating_sub(1));
    parts[..take].join("::")
}

fn find_qname(id: NodeId, merged: &MergedGraph) -> Option<String> {
    for g in &merged.graphs {
        if let Some(q) = g.nav.qname_by_id.get(&id) {
            return Some(q.clone());
        }
    }
    None
}

/// Emits edges that cross `RepoGraph` boundaries. v0.4.10 will add
/// `GraphQLResolver`, `GrpcResolver`, `QueueResolver`, etc. against the same
/// trait. Each resolver owns its own matching rule — path normalisation,
/// schema-name matching, queue-topic matching, etc.
pub trait CrossGraphResolver {
    fn resolve(&self, merged: &mut MergedGraph);
}

/// Pairs frontend HTTP Endpoints with backend HTTP Routes by (method,
/// normalised path) and emits `HTTP_CALLS` edges.
///
/// Matching rule:
/// - Endpoint qname `endpoint:<METHOD>:<path>` is the source side. Method comes
///   straight from the qname; path is normalised (see `normalise_http_path`).
/// - Route qname `route:<path>` — one Route node per path across all methods.
///   Methods live on stacked `ROUTE_METHOD` cells. Each (path, method) pair is
///   a distinct target.
/// - Cross-repo is the common case (Angular → Go gin backend), but same-repo
///   matches also link correctly (Next.js route-handlers + fetchers, etc.).
/// - Emitted edge confidence = min(endpoint_node_confidence, Strong) since
///   Routes are always Strong at v0.4.4 — i.e. the endpoint's confidence wins.
///
/// Collisions (multiple Routes with the same method+path across repos) emit
/// one edge per target. Rare in real corpora but cheap to handle.
pub struct HttpStackResolver;

impl CrossGraphResolver for HttpStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let index = build_route_index(&merged.graphs);
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ENDPOINT) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                let Some((method, raw_path)) = parse_endpoint_qname(qname) else {
                    continue;
                };
                if raw_path == "<unresolved>" {
                    continue;
                }
                let norm = normalise_http_path(raw_path);
                let targets = lookup_route_with_prefix_strip(&index, &method, &norm);
                for target in targets {
                    merged.cross_edges.push(Edge {
                        from: n.id,
                        to: target.route_id,
                        category: edge_category::HTTP_CALLS,
                        confidence: weakest(n.confidence, target.confidence),
                    });
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct RouteTarget {
    route_id: NodeId,
    confidence: Confidence,
}

/// Build `(METHOD, normalised_path) → Vec<RouteTarget>` across every graph in
/// the merge. One entry per `ROUTE_METHOD` cell found on each Route node.
fn build_route_index(
    graphs: &[RepoGraph],
) -> HashMap<(String, String), Vec<RouteTarget>> {
    let mut index: HashMap<(String, String), Vec<RouteTarget>> = HashMap::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ROUTE) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            let target = RouteTarget {
                route_id: n.id,
                confidence: n.confidence,
            };
            index_route_node(&mut index, qname, &n.cells, target);
        }
    }
    index
}

/// Register a ROUTE node into the (METHOD, path) index. Handles both qname
/// conventions now in the repo:
///   1. parser-go / ts_routes: qname = `route:<path>`, methods live on
///      stacked ROUTE_METHOD cells (JSON payload).
///   2. parser-java / parser-csharp / parser-rust / parser-php: qname =
///      `<METHOD> <path>`, one Route node per (method, path) with a single
///      ROUTE_METHOD cell carrying the method as a plain Text payload.
///
/// Both shapes target the same downstream key space so HttpStackResolver sees
/// all routes uniformly. Migrate the non-Go parsers to shape (1) when the
/// other resolvers start needing per-path aggregation.
fn index_route_node(
    index: &mut HashMap<(String, String), Vec<RouteTarget>>,
    qname: &str,
    cells: &[Cell],
    target: RouteTarget,
) {
    if let Some(path) = qname.strip_prefix("route:") {
        let norm = normalise_http_path(path);
        for cell in cells {
            if cell.kind != cell_type::ROUTE_METHOD {
                continue;
            }
            let Some(method) = cell_method(cell) else {
                continue;
            };
            index
                .entry((method.to_ascii_uppercase(), norm.clone()))
                .or_default()
                .push(target);
        }
        return;
    }
    // Legacy shape: "<METHOD> <path>". Split on the first space.
    if let Some((method, path)) = qname.split_once(' ')
        && path.starts_with('/')
    {
        let norm = normalise_http_path(path);
        index
            .entry((method.to_ascii_uppercase(), norm))
            .or_default()
            .push(target);
    }
}

/// Extract a method name from a ROUTE_METHOD cell, handling both the JSON
/// payload used by parser-go/ts_routes and the plain Text payload used by
/// parser-java/csharp/rust/php.
fn cell_method(cell: &Cell) -> Option<String> {
    match &cell.payload {
        CellPayload::Json(json) => extract_method_field(json).map(|s| s.to_string()),
        CellPayload::Text(s) => Some(s.clone()),
        CellPayload::Bytes(_) => None,
    }
}

fn parse_endpoint_qname(qname: &str) -> Option<(String, &str)> {
    let rest = qname.strip_prefix("endpoint:")?;
    let (method, path) = rest.split_once(':')?;
    Some((method.to_uppercase(), path))
}

/// Extract the `method` string field from a `ROUTE_METHOD` cell's JSON payload.
/// Minimal parse — the payload is a flat object written by parser-go, not
/// arbitrary user JSON, so a tight scan is enough and keeps us off serde_json
/// as a graph-crate dependency.
fn extract_method_field(json: &str) -> Option<&str> {
    let key = "\"method\"";
    let idx = json.find(key)?;
    let after = &json[idx + key.len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(&rest[..end])
}

/// Collapse path param syntaxes into a stable form so a frontend endpoint's
/// `/users/${id}` matches a backend route's `/users/:id` or `/users/{id}`.
/// Rules:
/// - Leading slash normalised to exactly one.
/// - Trailing slash stripped (except on the root).
/// - Segment matching `:x`, `{x}`, `${…}` (tree-sitter substitution marker),
///   or any segment containing `${` → `{}`.
/// - Empty segments collapse (so `//foo` → `/foo`).
pub fn normalise_http_path(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "/".to_string();
    }
    let body = trimmed.trim_matches('/');
    if body.is_empty() {
        return "/".to_string();
    }
    let segs: Vec<String> = body
        .split('/')
        .filter(|s| !s.is_empty())
        .map(normalise_segment)
        .collect();
    format!("/{}", segs.join("/"))
}

fn normalise_segment(seg: &str) -> String {
    if seg.starts_with(':')
        || (seg.starts_with('{') && seg.ends_with('}'))
        || seg.contains("${")
    {
        "{}".to_string()
    } else {
        seg.to_string()
    }
}

// Hardcoded for now — move to config.yaml if a real codebase needs a custom prefix.
const API_PREFIXES: &[&str] = &["protected", "api", "public", "internal", "v1", "v2", "v3"];

fn lookup_route_with_prefix_strip<'a>(
    index: &'a HashMap<(String, String), Vec<RouteTarget>>,
    method: &str,
    norm_path: &str,
) -> &'a [RouteTarget] {
    let key = (method.to_string(), norm_path.to_string());
    if let Some(targets) = index.get(&key) {
        return targets;
    }
    let segments: Vec<&str> = norm_path
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    for strip in 1..=2.min(segments.len().saturating_sub(1)) {
        if !API_PREFIXES.contains(&segments[strip - 1]) {
            break;
        }
        let stripped = format!("/{}", segments[strip..].join("/"));
        let key = (method.to_string(), stripped);
        if let Some(targets) = index.get(&key) {
            return targets;
        }
    }
    &[]
}

fn weakest(a: Confidence, b: Confidence) -> Confidence {
    fn rank(c: Confidence) -> u8 {
        match c {
            Confidence::Strong => 2,
            Confidence::Medium => 1,
            Confidence::Weak => 0,
        }
    }
    if rank(a) <= rank(b) { a } else { b }
}

// ============================================================================
// GrpcStackResolver — matches gRPC client → service by service name
// ============================================================================

pub struct GrpcStackResolver;

impl CrossGraphResolver for GrpcStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let index = build_grpc_service_index(&merged.graphs);
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::GRPC_CLIENT) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(svc_name) = qname.strip_prefix("grpc_client:") else { continue };
                let key = svc_name.split('.').next().unwrap_or(svc_name);
                if let Some(targets) = index.get(key) {
                    for t in targets {
                        merged.cross_edges.push(Edge {
                            from: n.id,
                            to: t.id,
                            category: edge_category::GRPC_CALLS,
                            confidence: weakest(n.confidence, t.confidence),
                        });
                    }
                }
            }
        }
    }
}

struct ServiceTarget {
    id: NodeId,
    confidence: Confidence,
}

fn build_grpc_service_index(graphs: &[RepoGraph]) -> HashMap<String, Vec<ServiceTarget>> {
    let mut index: HashMap<String, Vec<ServiceTarget>> = HashMap::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::GRPC_SERVICE) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some(svc_name) = qname.strip_prefix("grpc:") else { continue };
            index
                .entry(svc_name.to_string())
                .or_default()
                .push(ServiceTarget {
                    id: n.id,
                    confidence: n.confidence,
                });
        }
    }
    index
}

// ============================================================================
// QueueStackResolver — matches producer → consumer by topic name
// ============================================================================

pub struct QueueStackResolver;

impl CrossGraphResolver for QueueStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let consumer_index = build_queue_index(&merged.graphs, node_kind::QUEUE_CONSUMER, "queue_consumer:");
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::QUEUE_PRODUCER) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(topic) = qname.strip_prefix("queue_producer:") else { continue };
                if let Some(targets) = consumer_index.get(topic) {
                    for t in targets {
                        merged.cross_edges.push(Edge {
                            from: n.id,
                            to: t.id,
                            category: edge_category::QUEUE_FLOWS,
                            confidence: weakest(n.confidence, t.confidence),
                        });
                    }
                }
            }
        }
    }
}

fn build_queue_index(
    graphs: &[RepoGraph],
    kind: NodeKindId,
    prefix: &str,
) -> HashMap<String, Vec<ServiceTarget>> {
    let mut index: HashMap<String, Vec<ServiceTarget>> = HashMap::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&kind) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some(topic) = qname.strip_prefix(prefix) else { continue };
            index
                .entry(topic.to_string())
                .or_default()
                .push(ServiceTarget {
                    id: n.id,
                    confidence: n.confidence,
                });
        }
    }
    index
}

// ============================================================================
// GraphQLStackResolver — matches operation → resolver by name
// ============================================================================

pub struct GraphQLStackResolver;

impl CrossGraphResolver for GraphQLStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let resolver_index = build_kind_index(&merged.graphs, node_kind::GRAPHQL_RESOLVER, "graphql_resolver:");
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::GRAPHQL_OPERATION) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(op_name) = qname.strip_prefix("graphql_op:") else { continue };
                for (resolver_key, targets) in &resolver_index {
                    if names_match_graphql(op_name, resolver_key) {
                        for t in targets {
                            merged.cross_edges.push(Edge {
                                from: n.id,
                                to: t.id,
                                category: edge_category::GRAPHQL_CALLS,
                                confidence: weakest(n.confidence, t.confidence),
                            });
                        }
                    }
                }
            }
        }
    }
}

fn names_match_graphql(operation: &str, resolver: &str) -> bool {
    let op_lower = operation.to_lowercase();
    let res_lower = resolver.to_lowercase();
    op_lower == res_lower
        || op_lower.contains(&res_lower)
        || res_lower.contains(&op_lower)
}

// ============================================================================
// WebSocketStackResolver — matches WS client → handler by path
// ============================================================================

pub struct WebSocketStackResolver;

impl CrossGraphResolver for WebSocketStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let handler_index = build_kind_index(&merged.graphs, node_kind::WS_HANDLER, "ws:");
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::WS_CLIENT) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(client_path) = qname.strip_prefix("ws_client:") else { continue };
                for (handler_key, targets) in &handler_index {
                    if ws_paths_match(client_path, handler_key) {
                        for t in targets {
                            merged.cross_edges.push(Edge {
                                from: n.id,
                                to: t.id,
                                category: edge_category::WS_CONNECTS,
                                confidence: weakest(n.confidence, t.confidence),
                            });
                        }
                    }
                }
            }
        }
    }
}

fn ws_paths_match(client: &str, handler: &str) -> bool {
    let norm_c = client.trim_matches('/').to_lowercase();
    let norm_h = handler.trim_matches('/').to_lowercase();
    norm_c == norm_h
        || norm_c.ends_with(&norm_h)
        || norm_h.ends_with(&norm_c)
        || (norm_c == "ws" || norm_h == "ws" || norm_h == "default")
}

// ============================================================================
// EventBusResolver — matches event emitter → handler by event name
// ============================================================================

pub struct EventBusResolver;

impl CrossGraphResolver for EventBusResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let handler_index = build_kind_index(&merged.graphs, node_kind::EVENT_HANDLER, "event_handle:");
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::EVENT_EMITTER) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(event_name) = qname.strip_prefix("event_emit:") else { continue };
                if let Some(targets) = handler_index.get(event_name) {
                    for t in targets {
                        merged.cross_edges.push(Edge {
                            from: n.id,
                            to: t.id,
                            category: edge_category::EVENT_FLOWS,
                            confidence: weakest(n.confidence, t.confidence),
                        });
                    }
                }
            }
        }
    }
}

// ============================================================================
// SharedSchemaResolver — detects shared imports across repos
// ============================================================================

pub struct SharedSchemaResolver;

impl CrossGraphResolver for SharedSchemaResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut import_index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                    continue;
                }
                if let Some(children) = g.nav.children_of.get(&n.id) {
                    for &child in children {
                        if let Some(qname) = g.nav.qname_by_id.get(&child)
                            && is_schema_type(qname, g.nav.kind_by_id.get(&child).copied())
                        {
                            import_index
                                .entry(g.nav.name_by_id.get(&child).cloned().unwrap_or_default())
                                .or_default()
                                .push((child, g.repo, n.confidence));
                        }
                    }
                }
            }
        }

        for refs in import_index.values() {
            if refs.len() < 2 {
                continue;
            }
            let repos: HashSet<RepoId> = refs.iter().map(|(_, r, _)| *r).collect();
            if repos.len() < 2 {
                continue;
            }
            for i in 0..refs.len() {
                for j in (i + 1)..refs.len() {
                    if refs[i].1 != refs[j].1 {
                        merged.cross_edges.push(Edge {
                            from: refs[i].0,
                            to: refs[j].0,
                            category: edge_category::SHARES_SCHEMA,
                            confidence: weakest(
                                refs[i].2,
                                refs[j].2,
                            ),
                        });
                    }
                }
            }
        }
    }
}

// ============================================================================
// DbResolver — joins services that touch the same Table / Collection /
// NodeLabel by indexing DATA_ENTITY nodes by their full qname
// (`data_entity:<flavor>:<name>`) and pairing nodes that live in different
// repos. Mirrors SharedSchemaResolver's pairwise-pair shape; the qname's
// flavor segment ensures `users` (SQL table) and `User` (Mongoose model)
// don't collide.
// ============================================================================

pub struct DbResolver;

impl CrossGraphResolver for DbResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut entity_index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> =
            HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::DATA_ENTITY) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                entity_index
                    .entry(qname.clone())
                    .or_default()
                    .push((n.id, g.repo, n.confidence));
            }
        }
        for refs in entity_index.values() {
            if refs.len() < 2 {
                continue;
            }
            let repos: HashSet<RepoId> = refs.iter().map(|(_, r, _)| *r).collect();
            if repos.len() < 2 {
                continue;
            }
            for i in 0..refs.len() {
                for j in (i + 1)..refs.len() {
                    if refs[i].1 != refs[j].1 {
                        merged.cross_edges.push(Edge {
                            from: refs[i].0,
                            to: refs[j].0,
                            category: edge_category::SHARES_DATA_ENTITY,
                            confidence: weakest(refs[i].2, refs[j].2),
                        });
                    }
                }
            }
        }
    }
}

// ============================================================================
// CronResolver — pairs CRON_JOB nodes that share the full qname
// (`cron:<schedule>:<target>`) across different repos. Surfaces drift /
// accidental duplication of scheduled work; pairing on schedule alone is
// noise (e.g. five unrelated 4am jobs).
// ============================================================================

pub struct CronResolver;

impl CrossGraphResolver for CronResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::CRON_JOB) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                index
                    .entry(qname.clone())
                    .or_default()
                    .push((n.id, g.repo, n.confidence));
            }
        }
        for refs in index.values() {
            if refs.len() < 2 {
                continue;
            }
            let repos: HashSet<RepoId> = refs.iter().map(|(_, r, _)| *r).collect();
            if repos.len() < 2 {
                continue;
            }
            for i in 0..refs.len() {
                for j in (i + 1)..refs.len() {
                    if refs[i].1 != refs[j].1 {
                        merged.cross_edges.push(Edge {
                            from: refs[i].0,
                            to: refs[j].0,
                            category: edge_category::SHARES_CRON_SCHEDULE,
                            confidence: weakest(refs[i].2, refs[j].2),
                        });
                    }
                }
            }
        }
    }
}

// ============================================================================
// ConfigResolver — pairs CONFIG_KEY nodes with the same qname across repos.
// Same env-var name consumed/defined in 2+ services → SHARES_CONFIG edge.
// Useful drift signal (key renamed in one place but not another) and a
// substrate query "which services depend on DB_URL?".
// ============================================================================

pub struct ConfigResolver;

impl CrossGraphResolver for ConfigResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::CONFIG_KEY) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                index
                    .entry(qname.clone())
                    .or_default()
                    .push((n.id, g.repo, n.confidence));
            }
        }
        for refs in index.values() {
            if refs.len() < 2 {
                continue;
            }
            let repos: HashSet<RepoId> = refs.iter().map(|(_, r, _)| *r).collect();
            if repos.len() < 2 {
                continue;
            }
            for i in 0..refs.len() {
                for j in (i + 1)..refs.len() {
                    if refs[i].1 != refs[j].1 {
                        merged.cross_edges.push(Edge {
                            from: refs[i].0,
                            to: refs[j].0,
                            category: edge_category::SHARES_CONFIG,
                            confidence: weakest(refs[i].2, refs[j].2),
                        });
                    }
                }
            }
        }
    }
}

// ============================================================================
// IacResolver — pairs INFRA_RESOURCE nodes with the same qname
// (`infra:<kind>:<name>`) across repos. Captures cross-service
// container/manifest references — image built in repo A consumed by k8s
// manifest in repo B; same Service name appearing in compose for one repo and
// k8s for another (drift signal).
// ============================================================================

pub struct IacResolver;

impl CrossGraphResolver for IacResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::INFRA_RESOURCE) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                index
                    .entry(qname.clone())
                    .or_default()
                    .push((n.id, g.repo, n.confidence));
            }
        }
        for refs in index.values() {
            if refs.len() < 2 {
                continue;
            }
            let repos: HashSet<RepoId> = refs.iter().map(|(_, r, _)| *r).collect();
            if repos.len() < 2 {
                continue;
            }
            for i in 0..refs.len() {
                for j in (i + 1)..refs.len() {
                    if refs[i].1 != refs[j].1 {
                        merged.cross_edges.push(Edge {
                            from: refs[i].0,
                            to: refs[j].0,
                            category: edge_category::SHARES_INFRA_REF,
                            confidence: weakest(refs[i].2, refs[j].2),
                        });
                    }
                }
            }
        }
    }
}

// ============================================================================
// PackageResolver — pairs PACKAGE_DEP nodes with the same qname
// (`package:<ecosystem>:<name>`) across repos. Surfaces the "two services
// depend on the same package" signal that would otherwise need an external
// SCA tool. Cross-language reachability (the differentiator vs Endor / Snyk
// / Socket.dev) lives in v0.5+ — this resolver only emits the dependency
// substrate; per-symbol reachability layers on top of it.
// ============================================================================

pub struct PackageResolver;

impl CrossGraphResolver for PackageResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let mut index: HashMap<String, Vec<(NodeId, RepoId, Confidence)>> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::PACKAGE_DEP) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                    continue;
                };
                index
                    .entry(qname.clone())
                    .or_default()
                    .push((n.id, g.repo, n.confidence));
            }
        }
        for refs in index.values() {
            if refs.len() < 2 {
                continue;
            }
            let repos: HashSet<RepoId> = refs.iter().map(|(_, r, _)| *r).collect();
            if repos.len() < 2 {
                continue;
            }
            for i in 0..refs.len() {
                for j in (i + 1)..refs.len() {
                    if refs[i].1 != refs[j].1 {
                        merged.cross_edges.push(Edge {
                            from: refs[i].0,
                            to: refs[j].0,
                            category: edge_category::SHARES_DEPENDENCY,
                            confidence: weakest(refs[i].2, refs[j].2),
                        });
                    }
                }
            }
        }
    }
}

fn is_schema_type(qname: &str, kind: Option<NodeKindId>) -> bool {
    let schema_hints = [
        "Schema", "Validator", "Type", "Model", "Entity", "DTO",
        "Input", "Output", "Params", "Request", "Response",
    ];
    let is_type_kind = matches!(
        kind,
        Some(k) if k == node_kind::CLASS || k == node_kind::INTERFACE || k == node_kind::STRUCT
    );
    is_type_kind && schema_hints.iter().any(|h| qname.contains(h))
}

// ============================================================================
// CliInvocationResolver — matches CLI invocations → CLI commands
// ============================================================================

pub struct CliInvocationResolver;

impl CrossGraphResolver for CliInvocationResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let command_index = build_kind_index(&merged.graphs, node_kind::CLI_COMMAND, "cli:");
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::CLI_INVOCATION) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(tool) = qname.strip_prefix("cli_invoke:") else { continue };
                if let Some(targets) = command_index.get(tool) {
                    for t in targets {
                        merged.cross_edges.push(Edge {
                            from: n.id,
                            to: t.id,
                            category: edge_category::CLI_INVOKES,
                            confidence: weakest(n.confidence, t.confidence),
                        });
                    }
                }
            }
        }
    }
}

// ============================================================================
// Shared index builder
// ============================================================================

fn build_kind_index(
    graphs: &[RepoGraph],
    kind: NodeKindId,
    prefix: &str,
) -> HashMap<String, Vec<ServiceTarget>> {
    let mut index: HashMap<String, Vec<ServiceTarget>> = HashMap::new();
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&kind) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some(key) = qname.strip_prefix(prefix) else { continue };
            index
                .entry(key.to_string())
                .or_default()
                .push(ServiceTarget {
                    id: n.id,
                    confidence: n.confidence,
                });
        }
    }
    index
}

// ============================================================================
// Traversal primitives
// ============================================================================

impl RepoGraph {
    /// Outgoing neighbours of `id`: `(target, category)` pairs.
    pub fn neighbours(&self, id: NodeId) -> Vec<(NodeId, EdgeCategoryId)> {
        self.edges
            .iter()
            .filter(|e| e.from == id)
            .map(|e| (e.to, e.category))
            .collect()
    }

    /// Node ids reachable from `start` following edges in `follow` up to
    /// `max_depth`. Start node excluded.
    pub fn bfs(
        &self,
        start: NodeId,
        follow: &[EdgeCategoryId],
        max_depth: usize,
    ) -> Vec<NodeId> {
        let allow: HashSet<EdgeCategoryId> = follow.iter().copied().collect();
        let mut visited: HashSet<NodeId> = HashSet::from([start]);
        let mut out = Vec::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(start, 0)]);
        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for e in self.edges.iter().filter(|e| e.from == node) {
                if !allow.contains(&e.category) {
                    continue;
                }
                if visited.insert(e.to) {
                    out.push(e.to);
                    queue.push_back((e.to, depth + 1));
                }
            }
        }
        out
    }

    /// Backward BFS — node ids that can reach `sink` by following edges in
    /// `follow` in the reverse direction, up to `max_depth` hops. Sink
    /// excluded.
    ///
    /// Joern-flavored data-flow primitive. Pair it with `reachable_by` to ask
    /// "which of these candidate sources can reach this sink?" without
    /// materialising the full predecessor frontier when only a small set is
    /// of interest.
    ///
    /// Cost is O(E * depth) per call (linear scan over edges per visited
    /// node). For repeated queries against the same graph, build a reverse
    /// adjacency index out-of-band; this primitive is intentionally
    /// index-free so it composes with `MergedGraph::all_edges`.
    pub fn predecessors(
        &self,
        sink: NodeId,
        follow: &[EdgeCategoryId],
        max_depth: usize,
    ) -> Vec<NodeId> {
        let allow: HashSet<EdgeCategoryId> = follow.iter().copied().collect();
        let mut visited: HashSet<NodeId> = HashSet::from([sink]);
        let mut out = Vec::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(sink, 0)]);
        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for e in self.edges.iter().filter(|e| e.to == node) {
                if !allow.contains(&e.category) {
                    continue;
                }
                if visited.insert(e.from) {
                    out.push(e.from);
                    queue.push_back((e.from, depth + 1));
                }
            }
        }
        out
    }

    /// Joern-style `reachableBy` — the subset of `sources` that can reach
    /// `sink` through reverse traversal along `follow` within `max_depth`
    /// hops. Returns nodes in stable iteration order over `sources` so
    /// callers can rely on the result for ranking, not just membership.
    ///
    /// When `sources.len() << total predecessors`, prefer this over
    /// `predecessors` + manual intersection — it short-circuits the BFS as
    /// soon as every candidate is hit.
    pub fn reachable_by(
        &self,
        sink: NodeId,
        sources: &[NodeId],
        follow: &[EdgeCategoryId],
        max_depth: usize,
    ) -> Vec<NodeId> {
        if sources.is_empty() {
            return Vec::new();
        }
        let allow: HashSet<EdgeCategoryId> = follow.iter().copied().collect();
        let target_set: HashSet<NodeId> = sources.iter().copied().collect();
        let mut visited: HashSet<NodeId> = HashSet::from([sink]);
        let mut hit: HashSet<NodeId> = HashSet::new();
        let mut queue: VecDeque<(NodeId, usize)> = VecDeque::from([(sink, 0)]);
        while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_depth || hit.len() == target_set.len() {
                if hit.len() == target_set.len() {
                    break;
                }
                continue;
            }
            for e in self.edges.iter().filter(|e| e.to == node) {
                if !allow.contains(&e.category) {
                    continue;
                }
                if visited.insert(e.from) {
                    if target_set.contains(&e.from) {
                        hit.insert(e.from);
                    }
                    queue.push_back((e.from, depth + 1));
                }
            }
        }
        sources.iter().copied().filter(|s| hit.contains(s)).collect()
    }

    /// Walk `parent_of` from `id` to the top. Excludes `id` itself.
    pub fn parent_chain(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut cur = id;
        while let Some(parent) = self.nav.parent_of.get(&cur).copied() {
            out.push(parent);
            cur = parent;
        }
        out
    }

    /// Count nodes of a given kind.
    pub fn count_of_kind(&self, kind: NodeKindId) -> usize {
        self.nav
            .kind_by_id
            .values()
            .filter(|k| **k == kind)
            .count()
    }

    /// Spreading activation (PPR) over this repo's graph.
    pub fn activate(
        &self,
        seeds: &[NodeId],
        config: &repo_graph_activation::ActivationConfig,
    ) -> repo_graph_activation::ActivationResult {
        let node_ids: Vec<NodeId> = self.nodes.iter().map(|n| n.id).collect();
        repo_graph_activation::activate(&node_ids, &self.edges, seeds, config)
    }
}

impl MergedGraph {
    /// Spreading activation over the full merged graph (all repos + cross edges).
    pub fn activate(
        &self,
        seeds: &[NodeId],
        config: &repo_graph_activation::ActivationConfig,
    ) -> repo_graph_activation::ActivationResult {
        let node_ids: Vec<NodeId> = self
            .graphs
            .iter()
            .flat_map(|g| g.nodes.iter().map(|n| n.id))
            .collect();
        let edges: Vec<Edge> = self.all_edges().cloned().collect();
        repo_graph_activation::activate(&node_ids, &edges, seeds, config)
    }
}

// ============================================================================
// Code-domain activation defaults
// ============================================================================

/// Default `ActivationConfig` for code graphs. Weights: `calls` and
/// `http_calls` highest, `imports` medium, structural edges (`contains`,
/// `defines`) lowest. Direction forward (impact analysis default).
pub fn code_activation_defaults() -> repo_graph_activation::ActivationConfig {
    use repo_graph_activation::{ActivationConfig, Direction, Specificity};

    let mut weights = HashMap::new();
    weights.insert(edge_category::CALLS, 5.0);
    weights.insert(edge_category::HTTP_CALLS, 5.0);
    weights.insert(edge_category::GRPC_CALLS, 5.0);
    weights.insert(edge_category::GRAPHQL_CALLS, 5.0);
    weights.insert(edge_category::QUEUE_FLOWS, 4.0);
    weights.insert(edge_category::WS_CONNECTS, 4.0);
    weights.insert(edge_category::EVENT_FLOWS, 4.0);
    weights.insert(edge_category::CLI_INVOKES, 3.0);
    weights.insert(edge_category::HANDLED_BY, 4.0);
    weights.insert(edge_category::IMPORTS, 3.0);
    weights.insert(edge_category::USES, 3.0);
    weights.insert(edge_category::SHARES_SCHEMA, 2.0);
    weights.insert(edge_category::TESTS, 2.0);
    weights.insert(edge_category::INJECTS, 2.0);
    weights.insert(edge_category::DEFINES, 1.0);
    weights.insert(edge_category::CONTAINS, 1.0);
    weights.insert(edge_category::DOCUMENTS, 0.5);

    ActivationConfig {
        damping: 0.5,
        direction: Direction::Forward,
        edge_weights: weights,
        node_specificity: Specificity::None,
        top_k: 50,
        max_iterations: 100,
        epsilon: 1e-6,
    }
}

/// Task-tuned edge-weight presets over [`code_activation_defaults`] (WP-F /
/// GR-5). Same PPR engine, different lens on which relationships matter:
/// - `"repair"` upweights what buggy code actually touches (calls + data/config
///   access + tests).
/// - `"review"` upweights structural relationships a reviewer reasons over
///   (calls, tests, implements/inherits, return types).
/// - `"onboard"` upweights the high-level shape (entry points + module /
///   containment / docs).
/// - `"default"` (or any unknown profile) returns the defaults unchanged.
pub fn code_activation_profile(profile: &str) -> repo_graph_activation::ActivationConfig {
    let mut config = code_activation_defaults();
    let w = &mut config.edge_weights;
    match profile {
        "repair" => {
            w.insert(edge_category::CALLS, 8.0);
            w.insert(edge_category::USES, 6.0);
            w.insert(edge_category::ACCESSES_DATA, 6.0);
            w.insert(edge_category::READS_CONFIG, 5.0);
            w.insert(edge_category::IMPORTS, 5.0);
            w.insert(edge_category::TESTS, 4.0);
        }
        "review" => {
            w.insert(edge_category::CALLS, 6.0);
            w.insert(edge_category::TESTS, 6.0);
            w.insert(edge_category::IMPLEMENTS, 5.0);
            w.insert(edge_category::INHERITS_FROM, 5.0);
            w.insert(edge_category::RETURNS_TYPE, 4.0);
        }
        "onboard" => {
            w.insert(edge_category::CONTAINS, 5.0);
            w.insert(edge_category::IMPORTS, 5.0);
            w.insert(edge_category::HANDLED_BY, 6.0);
            w.insert(edge_category::DEFINES, 3.0);
            w.insert(edge_category::DOCUMENTS, 3.0);
        }
        _ => {}
    }
    config
}

// ============================================================================
// Signal resolution (WP-B / GR-2)
// ============================================================================

impl MergedGraph {
    /// Resolve a failure / change *signal* to seed node ids (GR-2 `locate`).
    /// `kind` is `"stacktrace"`, `"test"`, `"diff"`, or `"auto"` (sniff the
    /// shape). Unresolvable tokens are simply absent from the result. The
    /// sniffer and all frame/symbol/path → node-id logic live here in Rust so
    /// every consumer (repo-graph, Engram, neuropil) shares one resolver.
    pub fn resolve_signal(&self, text: &str, kind: &str) -> Vec<NodeId> {
        let kind = if kind == "auto" { sniff_signal_kind(text) } else { kind };
        let mut out: Vec<NodeId> = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        match kind {
            "stacktrace" => {
                for (file, line) in parse_stack_frames(text) {
                    if let Some(id) = self.resolve_frame(&file, line) {
                        if seen.insert(id) {
                            out.push(id);
                        }
                    }
                }
            }
            "diff" => {
                let frames = parse_diff_frames(text);
                if frames.is_empty() {
                    // Plain changed-file list (one path per line): seed every
                    // node in each named file.
                    for line in text.lines() {
                        let p = line.trim();
                        if p.is_empty() || !p.contains('.') {
                            continue;
                        }
                        for id in self.resolve_file(p) {
                            if seen.insert(id) {
                                out.push(id);
                            }
                        }
                    }
                } else {
                    for (file, line) in frames {
                        if let Some(id) = self.resolve_frame(&file, line) {
                            if seen.insert(id) {
                                out.push(id);
                            }
                        }
                    }
                }
            }
            "test" => {
                for id in self.resolve_test_ids(text) {
                    if seen.insert(id) {
                        out.push(id);
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// The single most specific node whose POSITION cell spans `line_1based` in
    /// a file whose basename matches `file`. Narrowest span wins (method over
    /// class over module).
    fn resolve_frame(&self, file: &str, line_1based: u32) -> Option<NodeId> {
        let base = basename(file);
        let line0 = line_1based.saturating_sub(1);
        let mut best: Option<(NodeId, u32)> = None;
        for g in &self.graphs {
            for n in &g.nodes {
                if let Some((f, s, e)) = position_of(n) {
                    if basename(&f) == base && line0 >= s && line0 <= e {
                        let width = e - s;
                        if best.map(|(_, w)| width < w).unwrap_or(true) {
                            best = Some((n.id, width));
                        }
                    }
                }
            }
        }
        best.map(|(id, _)| id)
    }

    /// Every node whose POSITION file basename matches `file` (a changed-file
    /// seed when there's no line). Sorted by id for determinism.
    fn resolve_file(&self, file: &str) -> Vec<NodeId> {
        let base = basename(file);
        let mut out = Vec::new();
        for g in &self.graphs {
            for n in &g.nodes {
                if let Some((f, _, _)) = position_of(n) {
                    if basename(&f) == base {
                        out.push(n.id);
                    }
                }
            }
        }
        out.sort_by_key(|id| id.0);
        out
    }

    /// Resolve test ids (pytest-style `path::Class::test_name`, Go
    /// `pkg::TestName`, etc.). Matches a node whose qname ends with the
    /// `::`-joined non-path segments; falls back to the bare test name.
    fn resolve_test_ids(&self, text: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for tok in text.split_whitespace() {
            if !tok.contains("::") {
                continue;
            }
            let segs: Vec<&str> = tok.split("::").collect();
            // Drop a leading path-like segment (file part): it has a '.' or '/'.
            let name_segs: Vec<&str> = segs
                .iter()
                .copied()
                .filter(|s| !s.is_empty() && !s.contains('.') && !s.contains('/'))
                .collect();
            let Some(last) = name_segs.last() else { continue };
            let suffix = format!("::{}", name_segs.join("::"));
            // Prefer a qname ending with the full ::-suffix; else the bare name.
            let mut matched: Option<NodeId> = None;
            for g in &self.graphs {
                for (id, qn) in &g.nav.qname_by_id {
                    if qn.ends_with(&suffix) || qn.as_str() == *last {
                        matched = Some(*id);
                        break;
                    }
                }
                if matched.is_some() {
                    break;
                }
            }
            let id = matched.or_else(|| self.resolve_name(last));
            if let Some(id) = id {
                if seen.insert(id) {
                    out.push(id);
                }
            }
        }
        out
    }
}

/// Decide which signal kind `text` is when the caller passes `"auto"`.
fn sniff_signal_kind(text: &str) -> &'static str {
    if text.contains("+++ ") || text.contains("--- a/") || text.contains("\n@@ ") {
        return "diff";
    }
    if (text.contains("File \"") && text.contains("line "))
        || text.contains(".go:")
        || text.contains("\n  at ")
    {
        return "stacktrace";
    }
    // A single bare token with `::` and no whitespace is a test id.
    let t = text.trim();
    if t.contains("::") && !t.chars().any(|c| c.is_whitespace()) {
        return "test";
    }
    // Otherwise try frame extraction; if that's empty the caller gets nothing.
    "stacktrace"
}

/// Extract `(file, line_1based)` frames from a stacktrace across languages:
/// Python `File "x", line N`, plus a generic `path.ext:line[:col]` scan that
/// covers Node/JS (`at f (path:line:col)`), Go (`\tpath:line`), and others.
fn parse_stack_frames(text: &str) -> Vec<(String, u32)> {
    let mut frames = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("File \"") {
            if let Some(end) = rest.find('"') {
                let file = &rest[..end];
                if let Some(lpos) = rest[end..].find("line ") {
                    let after = &rest[end + lpos + 5..];
                    let num: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
                    if let Ok(n) = num.parse::<u32>() {
                        frames.push((file.to_string(), n));
                        continue;
                    }
                }
            }
        }
        frames.extend(scan_path_line(line));
    }
    frames
}

/// Find `path.ext:line` occurrences in a line (path must carry an extension to
/// avoid matching `http://`, bare `host:port`, etc.).
fn scan_path_line(line: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for tok in line.split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ',') {
        let mut parts = tok.split(':');
        let path = parts.next().unwrap_or("");
        if path.is_empty() || !path.contains('.') || path.ends_with('.') {
            continue;
        }
        if let Some(num) = parts.next() {
            let digits: String = num.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(n) = digits.parse::<u32>() {
                out.push((path.to_string(), n));
            }
        }
    }
    out
}

/// Extract `(file, new_line)` frames from a unified diff: track the current
/// `+++ b/<file>` and the `@@ +c,d @@` new-file line counter, emitting a frame
/// per added line. Empty if `text` isn't a unified diff.
fn parse_diff_frames(text: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    let mut cur_file: Option<String> = None;
    let mut new_line: u32 = 0;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("+++ ") {
            let p = p.split('\t').next().unwrap_or(p).trim();
            let p = p.strip_prefix("b/").unwrap_or(p);
            cur_file = if p == "/dev/null" { None } else { Some(p.to_string()) };
            continue;
        }
        if line.starts_with("--- ") {
            continue;
        }
        if let Some(h) = line.strip_prefix("@@ ") {
            if let Some(plus) = h.split('+').nth(1) {
                let c: String = plus.chars().take_while(|c| c.is_ascii_digit()).collect();
                new_line = c.parse().unwrap_or(0);
            }
            continue;
        }
        let Some(file) = &cur_file else { continue };
        if line.starts_with('+') {
            out.push((file.clone(), new_line));
            new_line = new_line.saturating_add(1);
        } else if line.starts_with('-') {
            // deletion: does not advance the new-file line counter
        } else {
            new_line = new_line.saturating_add(1);
        }
    }
    out
}

/// Parse a node's POSITION cell into `(file, start_line, end_line)` (0-based
/// rows), or `None`. Hand-rolled so the graph crate stays serde-free.
fn position_of(node: &Node) -> Option<(String, u32, u32)> {
    for c in &node.cells {
        if c.kind == cell_type::POSITION {
            if let repo_graph_core::CellPayload::Json(j) = &c.payload {
                let file = json_str_field(j, "file")?;
                let start = json_num_field(j, "start_line")?;
                let end = json_num_field(j, "end_line")?;
                return Some((file, start, end));
            }
        }
    }
    None
}

fn json_str_field(json: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\":\"");
    let start = json.find(&marker)? + marker.len();
    let end = json[start..].find('"')? + start;
    Some(json[start..end].to_string())
}

fn json_num_field(json: &str, key: &str) -> Option<u32> {
    let marker = format!("\"{key}\":");
    let start = json.find(&marker)? + marker.len();
    let digits: String = json[start..].chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn basename(path: &str) -> &str {
    path.rsplit(|c: char| c == '/' || c == '\\').next().unwrap_or(path)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::GRAPH_TYPE;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://unit")
    }

    #[test]
    fn activation_profiles_shift_weights() {
        let def = code_activation_defaults();
        let repair = code_activation_profile("repair");
        let onboard = code_activation_profile("onboard");
        // repair upweights CALLS above the default.
        assert!(
            repair.edge_weights[&edge_category::CALLS]
                > def.edge_weights[&edge_category::CALLS]
        );
        // onboard upweights CONTAINS above the default.
        assert!(
            onboard.edge_weights[&edge_category::CONTAINS]
                > def.edge_weights[&edge_category::CONTAINS]
        );
        // Unknown / default profile == defaults.
        assert_eq!(
            code_activation_profile("default").edge_weights[&edge_category::CALLS],
            def.edge_weights[&edge_category::CALLS]
        );
        assert_eq!(
            code_activation_profile("nonsense").edge_weights[&edge_category::CALLS],
            def.edge_weights[&edge_category::CALLS]
        );
    }

    #[test]
    fn relative_import_resolution() {
        assert_eq!(resolve_module_reference("myapp::users", "helpers", 1), "myapp::helpers");
        assert_eq!(resolve_module_reference("a::b::c", "d", 2), "a::d");
        assert_eq!(resolve_module_reference("a::b", "c.d", 0), "c::d");
        assert_eq!(resolve_module_reference("a::b::c", "", 1), "a::b");
    }

    #[test]
    fn empty_repo_builds_cleanly() {
        let g = build_python(repo(), vec![]).unwrap();
        assert!(g.nodes.is_empty());
        assert!(g.edges.is_empty());
    }

    #[test]
    fn normalise_http_path_collapses_all_param_syntaxes() {
        assert_eq!(normalise_http_path("/users/:id"), "/users/{}");
        assert_eq!(normalise_http_path("/users/{id}"), "/users/{}");
        assert_eq!(normalise_http_path("/users/${…}"), "/users/{}");
        assert_eq!(normalise_http_path("/api/users/:id/posts/:pid"), "/api/users/{}/posts/{}");
        assert_eq!(normalise_http_path("users/list"), "/users/list");
        assert_eq!(normalise_http_path("/users/list/"), "/users/list");
        assert_eq!(normalise_http_path("//double//slash"), "/double/slash");
        assert_eq!(normalise_http_path("/"), "/");
        assert_eq!(normalise_http_path(""), "/");
    }

    #[test]
    fn parse_endpoint_qname_splits_method_and_path() {
        assert_eq!(
            parse_endpoint_qname("endpoint:GET:/users"),
            Some(("GET".to_string(), "/users"))
        );
        assert_eq!(
            parse_endpoint_qname("endpoint:POST:/api/login"),
            Some(("POST".to_string(), "/api/login"))
        );
        assert_eq!(parse_endpoint_qname("route:/users"), None);
    }

    #[test]
    fn extract_method_field_handles_ordering_and_whitespace() {
        let json = r#"{"method":"POST","handler":"h","file":"x.go","line":1,"col":2}"#;
        assert_eq!(extract_method_field(json), Some("POST"));
        let spaced = r#"{ "method" : "GET" , "line" : 0 }"#;
        assert_eq!(extract_method_field(spaced), Some("GET"));
    }

    #[test]
    fn weakest_confidence_is_min_rank() {
        assert_eq!(weakest(Confidence::Strong, Confidence::Strong), Confidence::Strong);
        assert_eq!(weakest(Confidence::Strong, Confidence::Medium), Confidence::Medium);
        assert_eq!(weakest(Confidence::Medium, Confidence::Weak), Confidence::Weak);
        assert_eq!(weakest(Confidence::Weak, Confidence::Strong), Confidence::Weak);
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

    /// Build a 4-node `RepoGraph` by hand: A → B → C plus D → C, all CALLS
    /// edges. Used to exercise the reverse-traversal primitives in isolation
    /// of any parser quirks.
    fn flow_graph() -> RepoGraph {
        let r = repo();
        let a = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::a");
        let b = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::b");
        let c = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::c");
        let d = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::d");
        let nodes = vec![
            Node { id: a, repo: r, confidence: Confidence::Strong, cells: vec![] },
            Node { id: b, repo: r, confidence: Confidence::Strong, cells: vec![] },
            Node { id: c, repo: r, confidence: Confidence::Strong, cells: vec![] },
            Node { id: d, repo: r, confidence: Confidence::Strong, cells: vec![] },
        ];
        let edges = vec![
            Edge { from: a, to: b, category: edge_category::CALLS, confidence: Confidence::Strong },
            Edge { from: b, to: c, category: edge_category::CALLS, confidence: Confidence::Strong },
            Edge { from: d, to: c, category: edge_category::CALLS, confidence: Confidence::Strong },
        ];
        let mut nav = CodeNav::default();
        nav.record(a, "a", "m::a", node_kind::FUNCTION, None);
        nav.record(b, "b", "m::b", node_kind::FUNCTION, None);
        nav.record(c, "c", "m::c", node_kind::FUNCTION, None);
        nav.record(d, "d", "m::d", node_kind::FUNCTION, None);
        RepoGraph {
            repo: r,
            nodes,
            edges,
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    fn graph_with_positioned_fn() -> (MergedGraph, NodeId) {
        let r = repo();
        let id = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m::foo::bar");
        let mut nav = CodeNav::default();
        nav.record(id, "bar", "m::foo::bar", node_kind::FUNCTION, None);
        let node = Node {
            id,
            repo: r,
            confidence: Confidence::Strong,
            cells: vec![Cell {
                kind: cell_type::POSITION,
                payload: CellPayload::Json(
                    r#"{"file":"foo/bar.py","start_line":10,"end_line":20}"#.into(),
                ),
            }],
        };
        let g = RepoGraph {
            repo: r,
            nodes: vec![node],
            edges: vec![],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        };
        (MergedGraph::new(vec![g]), id)
    }

    #[test]
    fn resolve_signal_matches_frames_diffs_and_tests() {
        let (m, id) = graph_with_positioned_fn();

        // Python stacktrace frame inside the node's span (line 15 ∈ [11,21] 1-based).
        let tb = "Traceback:\n  File \"/repo/foo/bar.py\", line 15, in bar\n    x.y()";
        assert_eq!(m.resolve_signal(tb, "stacktrace"), vec![id]);
        // auto-sniff routes it the same way.
        assert_eq!(m.resolve_signal(tb, "auto"), vec![id]);

        // Generic path:line (Node/Go style).
        assert_eq!(m.resolve_signal("at fn (foo/bar.py:16:3)", "stacktrace"), vec![id]);

        // Unified diff touching the file.
        let diff = "--- a/foo/bar.py\n+++ b/foo/bar.py\n@@ -14,1 +14,2 @@\n+    x = 1\n";
        assert_eq!(m.resolve_signal(diff, "diff"), vec![id]);

        // Plain changed-file list.
        assert_eq!(m.resolve_signal("foo/bar.py\n", "diff"), vec![id]);

        // pytest-style test id resolves by qname suffix.
        assert_eq!(m.resolve_signal("tests/test_x.py::bar", "test"), vec![id]);

        // A frame in a different file resolves to nothing.
        assert!(m
            .resolve_signal("File \"other.py\", line 15, in q", "stacktrace")
            .is_empty());
    }

    #[test]
    fn subset_keeps_only_requested_nodes_and_internal_edges() {
        // flow_graph: a->b->c, d->c (CALLS).
        let m = MergedGraph::new(vec![flow_graph()]);
        let a = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::a");
        let b = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::b");
        let c = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::c");

        let sub = m.subset(&[a, b]);
        let g = &sub.graphs[0];
        let ids: HashSet<NodeId> = g.nodes.iter().map(|n| n.id).collect();
        assert_eq!(ids, HashSet::from([a, b]));
        // a->b kept (both in subset); b->c dropped (c excluded).
        assert_eq!(g.edges.len(), 1);
        assert!(g.edges.iter().any(|e| e.from == a && e.to == b));
        // nav filtered to the kept nodes.
        assert!(g.nav.qname_by_id.contains_key(&a));
        assert!(!g.nav.qname_by_id.contains_key(&c));
    }

    #[test]
    fn predecessors_walks_backward_along_chosen_categories() {
        let g = flow_graph();
        let c = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::c");
        let a = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::a");
        let b = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::b");
        let d = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::d");

        let preds: HashSet<NodeId> = g
            .predecessors(c, &[edge_category::CALLS], 5)
            .into_iter()
            .collect();
        assert_eq!(preds, HashSet::from([a, b, d]));

        // Depth-bounded — depth=1 only finds direct predecessors of c.
        let direct: HashSet<NodeId> = g
            .predecessors(c, &[edge_category::CALLS], 1)
            .into_iter()
            .collect();
        assert_eq!(direct, HashSet::from([b, d]));

        // Wrong category yields nothing — proves filter is enforced.
        let none = g.predecessors(c, &[edge_category::IMPORTS], 5);
        assert!(none.is_empty());
    }

    /// Reproduces the `impact`/`trace` non-determinism: a framework component
    /// where the `CLASS` carries the edges and a `COMPONENT` marker shares its
    /// name *and* qname but has none. Resolution must always land on the
    /// connected `CLASS`, regardless of `HashMap` iteration order.
    fn dupe_name_graph() -> MergedGraph {
        let r = repo();
        let class = NodeId::from_parts(GRAPH_TYPE, r, node_kind::CLASS, "pkg::Dup");
        let comp = NodeId::from_parts(GRAPH_TYPE, r, node_kind::COMPONENT, "pkg::Dup");
        let run = NodeId::from_parts(GRAPH_TYPE, r, node_kind::METHOD, "pkg::Dup::run");
        let mut nav = CodeNav::default();
        nav.record(class, "Dup", "pkg::Dup", node_kind::CLASS, None);
        nav.record(comp, "Dup", "pkg::Dup", node_kind::COMPONENT, None);
        nav.record(run, "run", "pkg::Dup::run", node_kind::METHOD, Some(class));
        let g = RepoGraph {
            repo: r,
            nodes: vec![
                Node { id: class, repo: r, confidence: Confidence::Strong, cells: vec![] },
                Node { id: comp, repo: r, confidence: Confidence::Strong, cells: vec![] },
                Node { id: run, repo: r, confidence: Confidence::Strong, cells: vec![] },
            ],
            // Only the CLASS participates in an edge; the COMPONENT is edgeless.
            edges: vec![Edge {
                from: class,
                to: run,
                category: edge_category::CONTAINS,
                confidence: Confidence::Strong,
            }],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        };
        MergedGraph::new(vec![g])
    }

    #[test]
    fn resolve_name_prefers_connected_node_over_edgeless_marker() {
        let m = dupe_name_graph();
        let class = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "pkg::Dup");
        // Both `resolve_name` (by simple name) and `node_id_by_qname` (exact
        // qname) hit the duplicate; both must choose the connected CLASS.
        assert_eq!(m.resolve_name("Dup"), Some(class));
        assert_eq!(m.node_id_by_qname("pkg::Dup"), Some(class));
    }

    #[test]
    fn pick_primary_is_order_independent() {
        let m = dupe_name_graph();
        let class = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "pkg::Dup");
        let comp = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::COMPONENT, "pkg::Dup");
        // Same candidate set in either order → same winner. This is the
        // property HashMap iteration order used to violate across processes.
        assert_eq!(m.pick_primary(&[class, comp]), Some(class));
        assert_eq!(m.pick_primary(&[comp, class]), Some(class));
        assert_eq!(m.pick_primary(&[]), None);
    }

    #[test]
    fn qname_matches_are_sorted_by_node_id() {
        let m = dupe_name_graph();
        let got = m.qnames_exact("pkg::Dup");
        assert_eq!(got.len(), 2, "both same-qname nodes returned");
        let mut want = got.clone();
        want.sort_by_key(|id| id.0);
        assert_eq!(got, want, "qnames_exact must return a stable id-sorted order");

        // Substring search reaches all three (Dup, Dup marker, Dup::run).
        let sub = m.qnames_containing("pkg::Dup");
        assert_eq!(sub.len(), 3);
        let mut want_sub = sub.clone();
        want_sub.sort_by_key(|id| id.0);
        assert_eq!(sub, want_sub);
    }

    /// Build a single-node RepoGraph holding one DATA_ENTITY for `qname` in
    /// `repo_id`. Used to assemble cross-repo fixtures for DbResolver tests.
    fn graph_with_entity(repo_id: RepoId, qname: &str) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::DATA_ENTITY, qname);
        let mut nav = CodeNav::default();
        nav.record(
            id,
            qname.rsplit(':').next().unwrap_or(qname),
            qname,
            node_kind::DATA_ENTITY,
            None,
        );
        RepoGraph {
            repo: repo_id,
            nodes: vec![Node {
                id,
                repo: repo_id,
                confidence: Confidence::Medium,
                cells: vec![],
            }],
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    #[test]
    fn db_resolver_pairs_same_entity_across_repos() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_entity(repo_a, "data_entity:sql:users");
        let g_b = graph_with_entity(repo_b, "data_entity:sql:users");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);

        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_DATA_ENTITY)
            .collect();
        assert_eq!(edges.len(), 1, "expected one cross-repo SHARES_DATA_ENTITY edge");
    }

    #[test]
    fn db_resolver_does_not_pair_within_a_single_repo() {
        // Two DATA_ENTITY nodes with the same qname inside one repo would
        // already collapse via NodeId; even if duplicated, no cross-edge.
        let repo_a = RepoId(11);
        let g1 = graph_with_entity(repo_a, "data_entity:sql:users");
        let g2 = graph_with_entity(repo_a, "data_entity:sql:users");
        let mut merged = MergedGraph::new(vec![g1, g2]);
        merged.run(&DbResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_DATA_ENTITY),
            "must not emit SHARES_DATA_ENTITY when all matches share the same repo"
        );
    }

    fn graph_with_cron(repo_id: RepoId, qname: &str) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::CRON_JOB, qname);
        let mut nav = CodeNav::default();
        nav.record(
            id,
            qname.split(':').nth(1).unwrap_or(qname),
            qname,
            node_kind::CRON_JOB,
            None,
        );
        RepoGraph {
            repo: repo_id,
            nodes: vec![Node {
                id,
                repo: repo_id,
                confidence: Confidence::Medium,
                cells: vec![],
            }],
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    fn graph_with_package(repo_id: RepoId, qname: &str) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::PACKAGE_DEP, qname);
        let mut nav = CodeNav::default();
        nav.record(
            id,
            qname.rsplit(':').next().unwrap_or(qname),
            qname,
            node_kind::PACKAGE_DEP,
            None,
        );
        RepoGraph {
            repo: repo_id,
            nodes: vec![Node {
                id,
                repo: repo_id,
                confidence: Confidence::Medium,
                cells: vec![],
            }],
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    #[test]
    fn package_resolver_pairs_same_dep_across_repos() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_package(repo_a, "package:npm:react");
        let g_b = graph_with_package(repo_b, "package:npm:react");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&PackageResolver);
        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_DEPENDENCY)
            .collect();
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn package_resolver_keeps_ecosystems_separate() {
        // Same package name, different ecosystems → no pair.
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_package(repo_a, "package:npm:requests");
        let g_b = graph_with_package(repo_b, "package:pypi:requests");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&PackageResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_DEPENDENCY)
        );
    }

    fn graph_with_infra(repo_id: RepoId, qname: &str) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::INFRA_RESOURCE, qname);
        let mut nav = CodeNav::default();
        nav.record(
            id,
            qname.rsplit(':').next().unwrap_or(qname),
            qname,
            node_kind::INFRA_RESOURCE,
            None,
        );
        RepoGraph {
            repo: repo_id,
            nodes: vec![Node {
                id,
                repo: repo_id,
                confidence: Confidence::Medium,
                cells: vec![],
            }],
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    #[test]
    fn iac_resolver_pairs_same_image_across_repos() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_infra(repo_a, "infra:image:api");
        let g_b = graph_with_infra(repo_b, "infra:image:api");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&IacResolver);
        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_INFRA_REF)
            .collect();
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn iac_resolver_keeps_kinds_separate() {
        // `infra:service:api` vs `infra:deployment:api` — same name, different
        // kind, distinct qnames → no pairing.
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_infra(repo_a, "infra:service:api");
        let g_b = graph_with_infra(repo_b, "infra:deployment:api");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&IacResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_INFRA_REF)
        );
    }

    fn graph_with_config(repo_id: RepoId, qname: &str) -> RepoGraph {
        let id = NodeId::from_parts(GRAPH_TYPE, repo_id, node_kind::CONFIG_KEY, qname);
        let mut nav = CodeNav::default();
        nav.record(
            id,
            qname.rsplit(':').next().unwrap_or(qname),
            qname,
            node_kind::CONFIG_KEY,
            None,
        );
        RepoGraph {
            repo: repo_id,
            nodes: vec![Node {
                id,
                repo: repo_id,
                confidence: Confidence::Medium,
                cells: vec![],
            }],
            edges: vec![],
            symbols: SymbolTable::default(),
            nav,
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: HashSet::new(),
        }
    }

    #[test]
    fn config_resolver_pairs_same_key_across_repos() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_config(repo_a, "config:env:DATABASE_URL");
        let g_b = graph_with_config(repo_b, "config:env:DATABASE_URL");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&ConfigResolver);
        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_CONFIG)
            .collect();
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn config_resolver_does_not_pair_distinct_keys() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_config(repo_a, "config:env:DATABASE_URL");
        let g_b = graph_with_config(repo_b, "config:env:API_KEY");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&ConfigResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_CONFIG)
        );
    }

    #[test]
    fn cron_resolver_pairs_same_schedule_target_across_repos() {
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_cron(repo_a, "cron:0 4 * * *:cleanup");
        let g_b = graph_with_cron(repo_b, "cron:0 4 * * *:cleanup");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&CronResolver);
        let edges: Vec<&Edge> = merged
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SHARES_CRON_SCHEDULE)
            .collect();
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn cron_resolver_does_not_pair_on_schedule_alone() {
        // Same schedule, different targets — must NOT pair (no drift).
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_cron(repo_a, "cron:0 4 * * *:cleanup");
        let g_b = graph_with_cron(repo_b, "cron:0 4 * * *:reindex");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&CronResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_CRON_SCHEDULE),
            "different targets at same schedule must not pair"
        );
    }

    #[test]
    fn db_resolver_keeps_flavors_separate() {
        // A SQL `users` table and a NoSQL `users` collection have different
        // qname flavor segments and must not be joined.
        let repo_a = RepoId(11);
        let repo_b = RepoId(12);
        let g_a = graph_with_entity(repo_a, "data_entity:sql:users");
        let g_b = graph_with_entity(repo_b, "data_entity:nosql:users");
        let mut merged = MergedGraph::new(vec![g_a, g_b]);
        merged.run(&DbResolver);
        assert!(
            merged
                .cross_edges
                .iter()
                .all(|e| e.category != edge_category::SHARES_DATA_ENTITY),
            "flavor mismatch must not emit a SHARES_DATA_ENTITY edge"
        );
    }

    #[test]
    fn reachable_by_intersects_predecessors_with_sources() {
        let g = flow_graph();
        let c = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::c");
        let a = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::a");
        let d = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::d");
        let absent =
            NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "m::ghost");

        // a (transitive via b) and d (direct) both reach c. ghost is not in graph.
        let hit = g.reachable_by(c, &[a, d, absent], &[edge_category::CALLS], 5);
        assert_eq!(hit, vec![a, d]);

        // Source order is preserved (d listed first → d listed first).
        let hit_swapped = g.reachable_by(c, &[d, a], &[edge_category::CALLS], 5);
        assert_eq!(hit_swapped, vec![d, a]);

        // Empty sources short-circuits.
        assert!(g.reachable_by(c, &[], &[edge_category::CALLS], 5).is_empty());
    }

    // ------------------------------------------------------------------------
    // G17 / G19 / G20 / G22 — MergedGraph helpers
    // ------------------------------------------------------------------------

    fn synth_repo_graph(repo: RepoId, items: &[(&str, NodeKindId)]) -> RepoGraph {
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
        for (qname, kind) in items {
            let id = NodeId::from_parts(GRAPH_TYPE, repo, *kind, qname);
            g.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: Vec::new(),
            });
            let leaf = qname.rsplit("::").next().unwrap_or(qname);
            g.nav.record(id, leaf, qname, *kind, None);
        }
        g
    }

    #[test]
    fn for_each_node_of_kind_walks_every_repo() {
        let r1 = RepoId::from_canonical("test://r1");
        let r2 = RepoId::from_canonical("test://r2");
        let g1 = synth_repo_graph(
            r1,
            &[
                ("a::A", node_kind::CLASS),
                ("a::B", node_kind::CLASS),
                ("a::f", node_kind::FUNCTION),
            ],
        );
        let g2 = synth_repo_graph(
            r2,
            &[
                ("b::C", node_kind::CLASS),
                ("b::g", node_kind::FUNCTION),
            ],
        );
        let merged = MergedGraph::new(vec![g1, g2]);
        let classes = merged.nodes_of_kind(node_kind::CLASS);
        assert_eq!(classes.len(), 3);
        let funcs = merged.nodes_of_kind(node_kind::FUNCTION);
        assert_eq!(funcs.len(), 2);
    }

    #[test]
    fn kind_index_groups_by_kind() {
        let r = RepoId::from_canonical("test://idx");
        let g = synth_repo_graph(
            r,
            &[
                ("m::A", node_kind::CLASS),
                ("m::B", node_kind::CLASS),
                ("m::f", node_kind::FUNCTION),
            ],
        );
        let merged = MergedGraph::new(vec![g]);
        let idx = merged.kind_index();
        assert_eq!(idx.get(&node_kind::CLASS).map(|v| v.len()), Some(2));
        assert_eq!(idx.get(&node_kind::FUNCTION).map(|v| v.len()), Some(1));
    }

    #[test]
    fn node_id_by_qname_resolves_across_repos() {
        let r1 = RepoId::from_canonical("test://r1");
        let r2 = RepoId::from_canonical("test://r2");
        let g1 = synth_repo_graph(r1, &[("a::X", node_kind::CLASS)]);
        let g2 = synth_repo_graph(r2, &[("b::Y", node_kind::CLASS)]);
        let merged = MergedGraph::new(vec![g1, g2]);
        let want_y = NodeId::from_parts(GRAPH_TYPE, r2, node_kind::CLASS, "b::Y");
        assert_eq!(merged.node_id_by_qname("b::Y"), Some(want_y));
        assert_eq!(merged.node_id_by_qname("nope"), None);
    }

    #[test]
    fn resolve_span_matches_dotted_and_suffix() {
        let r = RepoId::from_canonical("test://span");
        let g = synth_repo_graph(
            r,
            &[
                ("svc::handlers::users::list_users", node_kind::FUNCTION),
                ("other::list_users", node_kind::FUNCTION),
            ],
        );
        let merged = MergedGraph::new(vec![g]);
        let exact = merged.resolve_span("svc.handlers.users.list_users");
        assert!(exact.is_some());
        let suffix = merged.resolve_span("handlers.users.list_users");
        assert!(suffix.is_some());
        let miss = merged.resolve_span("nonexistent");
        assert_eq!(miss, None);
    }

    #[test]
    fn cluster_key_for_drops_leaf_segment() {
        let r = RepoId::from_canonical("test://ck");
        let g = synth_repo_graph(
            r,
            &[
                ("svc::users::repo::find_one", node_kind::METHOD),
                ("svc::users::repo", node_kind::CLASS),
                ("solo", node_kind::MODULE),
            ],
        );
        let merged = MergedGraph::new(vec![g]);
        let find_one = NodeId::from_parts(
            GRAPH_TYPE,
            r,
            node_kind::METHOD,
            "svc::users::repo::find_one",
        );
        assert_eq!(cluster_key_for(find_one, 0, &merged), "");
        assert_eq!(cluster_key_for(find_one, 1, &merged), "svc");
        assert_eq!(cluster_key_for(find_one, 2, &merged), "svc::users");
        // Depth caps at parts-1; leaf never lands in the key.
        assert_eq!(
            cluster_key_for(find_one, 99, &merged),
            "svc::users::repo"
        );
        // Single-segment qname → empty key (no parent cluster).
        let solo = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, "solo");
        assert_eq!(cluster_key_for(solo, 1, &merged), "");
        // Unknown node id → empty.
        assert_eq!(cluster_key_for(NodeId(0xdeadbeef), 1, &merged), "");
    }
}
