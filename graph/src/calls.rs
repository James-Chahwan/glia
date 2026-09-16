//! Call / ref resolution and the nav-walking helpers it needs.

use std::collections::HashMap;

use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, UnresolvedRef, edge_category, node_kind,
};
use repo_graph_core::{Confidence, Edge, EdgeCategoryId, NodeId};

use crate::types::RepoGraph;

// ============================================================================
// Call resolution
// ============================================================================

/// Cross-file call resolution — same recipe for all languages.
///
/// `extra_hook` is an escape hatch for language-specific resolution shapes
/// that the generic pass doesn't cover. Unused today (pass `|_, _| None`);
/// it's the seam for future Go method-on-struct-via-package-alias lookups
/// and similar language-specific call shapes.
pub(crate) fn resolve_calls<H>(g: &mut RepoGraph, calls: &[CallSite], extra_hook: H)
where
    H: Fn(&RepoGraph, &CallSite) -> Option<NodeId>,
{
    let mut pkg_base_bound = 0usize;
    for site in calls {
        let Some(from_module) = enclosing_module(&g.nav, site.from) else {
            g.unresolved_calls.push(site.clone());
            continue;
        };
        let bindings = g.symbols.module_import_bindings.get(&from_module);

        let resolved: Option<NodeId> = match &site.qualifier {
            CallQualifier::Bare(name) => {
                // Priority: local import binding → same-module top-level def →
                // enclosing PACKAGE's def (Elixir: `def`s live under a defmodule
                // PACKAGE, not the file MODULE).
                bindings
                    .and_then(|b| b.get(name).copied())
                    .or_else(|| {
                        g.symbols
                            .module_symbols
                            .get(&from_module)
                            .and_then(|s| s.get(name).copied())
                    })
                    .or_else(|| {
                        enclosing_package(&g.nav, site.from).and_then(|pkg| {
                            g.symbols.module_symbols.get(&pkg).and_then(|s| s.get(name).copied())
                        })
                    })
            }
            CallQualifier::Attribute { base, name } => {
                let hit = resolve_attribute_target(g, bindings, base, name);
                if hit.is_some()
                    && attribute_base_kind(g, bindings, base) == Some(node_kind::PACKAGE)
                {
                    pkg_base_bound += 1;
                }
                hit
            }
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
    if pkg_base_bound > 0 {
        eprintln!("[resolve] package-base attribute calls bound: {pkg_base_bound}");
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
pub(crate) fn resolve_refs(g: &mut RepoGraph, refs: &[UnresolvedRef]) {
    let mut pkg_base_bound = 0usize;
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
                    // Global-by-name fallback. HANDLED_BY: route handler is a
                    // same-package fn (import binding can't see it). INJECTS
                    // (Pattern E): the injected service TYPE is resolved by its
                    // unique class name across the repo — DI param types are
                    // often unresolvable via imports (TS/dotted imports are a
                    // separate gap), and `module_symbols` registers top-level
                    // classes by name, so a uniquely-named service binds here.
                    if r.category == edge_category::HANDLED_BY
                        || r.category == edge_category::INJECTS
                        || r.category == edge_category::INHERITS_FROM
                        || r.category == edge_category::IMPLEMENTS
                    {
                        // Heritage (INHERITS_FROM/IMPLEMENTS) and DI (INJECTS) name a
                        // type by its bare name; resolve to the uniquely-named class/
                        // interface across the repo (module_symbols indexes them,
                        // incl. namespace/PACKAGE members). Ambiguity → None.
                        unique_global_function(g, name)
                    } else {
                        None
                    }
                }),
            CallQualifier::Attribute { base, name } => {
                let hit = resolve_attribute_target(g, bindings, base, name);
                if hit.is_some()
                    && attribute_base_kind(g, bindings, base) == Some(node_kind::PACKAGE)
                {
                    pkg_base_bound += 1;
                }
                // Global fallback for HANDLED_BY: in Go, route handlers
                // are usually written `h.GetProfile` where `h` is a local
                // struct-receiver variable (`h *Handlers`), not an import
                // binding. So binding lookup fails. Scan all class_methods
                // across the graph for a method matching `name`; emit
                // only when exactly one match exists.
                hit.or_else(|| {
                    if r.category == edge_category::HANDLED_BY {
                        unique_global_method(g, name)
                    } else {
                        None
                    }
                })
            }
            CallQualifier::SelfMethod(_)
            | CallQualifier::SuperMethod(_)
            | CallQualifier::ComplexReceiver { .. } => None,
        };

        match resolved {
            Some(to) => push_edge(g, r.from, to, r.category),
            None => g.unresolved_refs.push(r.clone()),
        }
    }
    if pkg_base_bound > 0 {
        eprintln!("[resolve] package-base attribute calls bound: {pkg_base_bound}");
    }
}

/// Resolve `base.name()` where `base` is a plain identifier already bound in
/// this module's import table. MODULE and PACKAGE bases both scope top-level
/// defs (`build_symbol_table` indexes both into `module_symbols`), so an Elixir
/// `alias MyApp.Accounts` + `Accounts.get_user(id)` resolves through the
/// `defmodule` PACKAGE node; CLASS and STRUCT bases scope methods.
fn resolve_attribute_target(
    g: &RepoGraph,
    bindings: Option<&HashMap<String, NodeId>>,
    base: &str,
    name: &str,
) -> Option<NodeId> {
    let base_id = bindings?.get(base).copied()?;
    match g.nav.kind_by_id.get(&base_id).copied() {
        Some(k) if k == node_kind::MODULE || k == node_kind::PACKAGE => {
            g.symbols.module_symbols.get(&base_id).and_then(|s| s.get(name).copied())
        }
        Some(k) if k == node_kind::CLASS || k == node_kind::STRUCT => {
            g.symbols.class_methods.get(&base_id).and_then(|m| m.get(name).copied())
        }
        _ => None,
    }
}

/// Kind of the node `base` is bound to in this module's import table — used
/// only to attribute the `[resolve] package-base` counter.
fn attribute_base_kind(
    g: &RepoGraph,
    bindings: Option<&HashMap<String, NodeId>>,
    base: &str,
) -> Option<repo_graph_core::NodeKindId> {
    let base_id = bindings?.get(base)?;
    g.nav.kind_by_id.get(base_id).copied()
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

/// A module/package whose qname's final segment equals `tail`, iff unique.
/// For imports that target a module/namespace (`import app.util`, Go/Clojure/
/// Elixir) where the file layout doesn't produce the full dotted qname, so the
/// import binds by the module's short name. Deduped by id; None on ambiguity.
pub(crate) fn unique_global_module(g: &RepoGraph, tail: &str) -> Option<NodeId> {
    let mut hit: Option<NodeId> = None;
    for (qname, &id) in &g.symbols.module_by_qname {
        if qname.rsplit("::").next() == Some(tail) {
            match hit {
                Some(existing) if existing == id => {}
                Some(_) => return None,
                None => hit = Some(id),
            }
        }
    }
    hit
}

/// Same idea for top-level functions across the repo.
pub(crate) fn unique_global_function(g: &RepoGraph, name: &str) -> Option<NodeId> {
    let mut hit: Option<NodeId> = None;
    for syms in g.symbols.module_symbols.values() {
        if let Some(&id) = syms.get(name) {
            match hit {
                // Same node registered under both its MODULE and its PACKAGE
                // (namespace) — not ambiguous, it's one node.
                Some(existing) if existing == id => {}
                Some(_) => return None, // two distinct nodes share the name
                None => hit = Some(id),
            }
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

/// Nearest enclosing PACKAGE (namespace / defmodule). Elixir `def`s live under a
/// `defmodule` PACKAGE, not the file MODULE, so a bare sibling call resolves via
/// the package's symbols. Additive fallback — `module_symbols` indexes PACKAGE
/// members (build_symbol_table).
fn enclosing_package(nav: &CodeNav, mut id: NodeId) -> Option<NodeId> {
    loop {
        id = *nav.parent_of.get(&id)?;
        if nav.kind_by_id.get(&id) == Some(&node_kind::PACKAGE) {
            return Some(id);
        }
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

pub(crate) fn push_edge(g: &mut RepoGraph, from: NodeId, to: NodeId, category: EdgeCategoryId) {
    g.edges.push(Edge {
        from,
        to,
        category,
        confidence: Confidence::Strong,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::build_dotted;
    use crate::test_support::repo;
    use repo_graph_code_domain::{FileParse, GRAPH_TYPE, ImportStmt, ImportTarget};
    use repo_graph_core::Node;
    use std::collections::HashSet;

    /// Elixir shape: `defmodule` emits a PACKAGE node holding the `def`s, and
    /// `alias MyApp.Accounts` binds that PACKAGE by its short name. Before the
    /// PACKAGE arm in `resolve_attribute_target`, `Accounts.get_user(id)` had a
    /// bound base but an unaccepted base KIND, so it fell into unresolved_calls.
    #[test]
    fn attribute_call_binds_package_base() {
        let r = repo();
        let m2 = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, "m2");
        let pkg = NodeId::from_parts(GRAPH_TYPE, r, node_kind::PACKAGE, "m2::MyApp.Accounts");
        let callee =
            NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m2::MyApp.Accounts::get_user");
        let m1 = NodeId::from_parts(GRAPH_TYPE, r, node_kind::MODULE, "m1");
        let caller = NodeId::from_parts(GRAPH_TYPE, r, node_kind::FUNCTION, "m1::show");

        let mut nav2 = CodeNav::default();
        nav2.record(m2, "m2", "m2", node_kind::MODULE, None);
        nav2.record(pkg, "Accounts", "m2::MyApp.Accounts", node_kind::PACKAGE, Some(m2));
        nav2.record(
            callee,
            "get_user",
            "m2::MyApp.Accounts::get_user",
            node_kind::FUNCTION,
            Some(pkg),
        );
        let node = |id| Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] };
        let accounts = FileParse {
            nodes: vec![node(m2), node(pkg), node(callee)],
            edges: vec![],
            imports: vec![],
            calls: vec![],
            refs: vec![],
            nav: nav2,
            properties: HashSet::new(),
        };

        let mut nav1 = CodeNav::default();
        nav1.record(m1, "m1", "m1", node_kind::MODULE, None);
        nav1.record(caller, "show", "m1::show", node_kind::FUNCTION, Some(m1));
        let controller = FileParse {
            nodes: vec![node(m1), node(caller)],
            edges: vec![],
            imports: vec![ImportStmt {
                from_module: "m1".to_string(),
                target: ImportTarget::Module {
                    path: "MyApp.Accounts".to_string(),
                    alias: None,
                },
            }],
            calls: vec![CallSite {
                from: caller,
                qualifier: CallQualifier::Attribute {
                    base: "Accounts".to_string(),
                    name: "get_user".to_string(),
                },
            }],
            refs: vec![],
            nav: nav1,
            properties: HashSet::new(),
        };

        let g = build_dotted(r, vec![accounts, controller]).unwrap();
        let calls: Vec<_> =
            g.edges.iter().filter(|e| e.category == edge_category::CALLS).collect();
        assert_eq!(calls.len(), 1, "expected exactly one CALLS edge, got {calls:?}");
        assert_eq!(calls[0].from, caller);
        assert_eq!(calls[0].to, callee);
        assert!(g.unresolved_calls.is_empty(), "call should not land in unresolved_calls");
    }
}
