//! Call / ref resolution and the nav-walking helpers it needs.

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
pub(crate) fn resolve_refs(g: &mut RepoGraph, refs: &[UnresolvedRef]) {
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
