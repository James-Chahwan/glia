//! Call / ref resolution and the nav-walking helpers it needs.

use std::collections::HashMap;

use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, UnresolvedRef, cell_type, edge_category, node_kind,
    recv_stats,
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
    let mut enum_hits = EnumHits::default();
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
                if hit.is_some() {
                    match attribute_base(g, bindings, base) {
                        Some((_, k)) if k == node_kind::PACKAGE => pkg_base_bound += 1,
                        Some((base_id, k)) if k == node_kind::ENUM => {
                            enum_hits.attribute += 1;
                            enum_hits.enums.push(base_id);
                        }
                        _ => {}
                    }
                }
                hit
            }
            CallQualifier::SelfMethod(name) => {
                let owner = enclosing_class_or_struct(&g.nav, site.from);
                let hit = owner.and_then(|parent_id| {
                    g.symbols
                        .class_methods
                        .get(&parent_id)
                        .and_then(|m| m.get(name).copied())
                });
                if let (Some(owner_id), Some(_)) = (owner, hit)
                    && g.nav.kind_by_id.get(&owner_id) == Some(&node_kind::ENUM)
                {
                    enum_hits.self_method += 1;
                    enum_hits.enums.push(owner_id);
                }
                hit
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

        // A6.2a: `_repo.Find()` / `this.repo.find()` where the receiver is a
        // declared field of the enclosing type. Strictly AFTER every lookup
        // above, so an import binding or module symbol that shares the
        // field's name keeps today's target.
        let resolved = resolved.or_else(|| {
            let hit = resolve_via_receiver_type(g, site, from_module);
            if hit.is_some() {
                recv_stats::record();
            }
            hit
        });

        let resolved = resolved.or_else(|| extra_hook(g, site));

        match resolved {
            Some(to) => push_edge(g, site.from, to, edge_category::CALLS),
            None => g.unresolved_calls.push(site.clone()),
        }
    }
    if pkg_base_bound > 0 {
        eprintln!("[resolve] package-base attribute calls bound: {pkg_base_bound}");
    }
    if enum_hits.self_method + enum_hits.attribute > 0 {
        eprintln!(
            "[resolve] enum-owned calls bound: self_method={} attribute={} ext={}",
            enum_hits.self_method,
            enum_hits.attribute,
            enum_hits.ext(g)
        );
    }
}

/// Per-build tally of resolutions that only bind because an ENUM owns methods
/// and members (LA.30a). `enums` holds every ENUM that received a hit; the
/// marker's `ext` discriminator is read from the lowest-qname one, since the
/// generic pass does not know which language it is building.
#[derive(Default)]
struct EnumHits {
    self_method: usize,
    attribute: usize,
    uses: usize,
    enums: Vec<NodeId>,
}

impl EnumHits {
    /// File extension of the POSITION cell of the lowest-qname ENUM hit, or `?`
    /// when that ENUM carries no POSITION (or its file has no extension).
    fn ext(&self, g: &RepoGraph) -> String {
        // NodeId is Hash, not Ord: order by qname alone. Two hits with the same
        // ENUM qname are the same node (the id derives from kind + qname).
        let lowest = self
            .enums
            .iter()
            .filter_map(|id| g.nav.qname_by_id.get(id).map(|q| (q.as_str(), *id)))
            .min_by(|a, b| a.0.cmp(b.0));
        lowest
            .and_then(|(_, id)| g.nodes.iter().find(|n| n.id == id))
            .and_then(position_file)
            .and_then(|file| {
                let base = file.rsplit('/').next().unwrap_or(&file);
                base.rsplit_once('.').map(|(_, ext)| ext.to_string())
            })
            .unwrap_or_else(|| "?".to_string())
    }
}

/// The `file` field of a node's POSITION cell (JSON `{"file":"…",…}`).
fn position_file(node: &repo_graph_core::Node) -> Option<String> {
    node.cells.iter().find_map(|c| {
        if c.kind != cell_type::POSITION {
            return None;
        }
        let repo_graph_core::CellPayload::Json(j) = &c.payload else {
            return None;
        };
        let marker = "\"file\":\"";
        let start = j.find(marker)? + marker.len();
        let end = j[start..].find('"')? + start;
        Some(j[start..end].to_string())
    })
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
    let mut enum_hits = EnumHits::default();
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
                let bound_base = attribute_base(g, bindings, base);
                let hit = resolve_attribute_target(g, bindings, base, name);
                if hit.is_some() && bound_base.map(|(_, k)| k) == Some(node_kind::PACKAGE) {
                    pkg_base_bound += 1;
                }
                // `Enum.MEMBER` / `Enum::Variant` read as a USES ref: bind the
                // ENUM's own ATTRIBUTE child. USES-only, so a call site
                // `Color.RED()` (CALLS) can never land on a member.
                let hit = hit.or_else(|| match bound_base {
                    Some((base_id, k))
                        if k == node_kind::ENUM && r.category == edge_category::USES =>
                    {
                        let member = enum_member(g, base_id, name);
                        if member.is_some() {
                            enum_hits.uses += 1;
                            enum_hits.enums.push(base_id);
                        }
                        member
                    }
                    _ => None,
                });
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
    if enum_hits.uses > 0 {
        eprintln!("[resolve] enum member uses bound: {} ext={}", enum_hits.uses, enum_hits.ext(g));
    }
}

/// Resolve `base.name()` where `base` is a plain identifier already bound in
/// this module's import table. MODULE and PACKAGE bases both scope top-level
/// defs (`build_symbol_table` indexes both into `module_symbols`), so an Elixir
/// `alias MyApp.Accounts` + `Accounts.get_user(id)` resolves through the
/// `defmodule` PACKAGE node; CLASS, STRUCT and ENUM bases scope methods (an
/// ENUM owns its methods exactly like a class — `Color.pick()`).
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
        Some(k) if k == node_kind::CLASS || k == node_kind::STRUCT || k == node_kind::ENUM => {
            g.symbols.class_methods.get(&base_id).and_then(|m| m.get(name).copied())
        }
        _ => None,
    }
}

/// The node `base` is bound to in this module's import table, with its kind —
/// attributes the `[resolve]` counters and gates the ENUM-member lookup.
fn attribute_base(
    g: &RepoGraph,
    bindings: Option<&HashMap<String, NodeId>>,
    base: &str,
) -> Option<(NodeId, repo_graph_core::NodeKindId)> {
    let base_id = *bindings?.get(base)?;
    g.nav.kind_by_id.get(&base_id).map(|k| (base_id, *k))
}

/// `Enum.MEMBER` / `Enum::Variant`: the ATTRIBUTE child of `enum_id` named
/// `name`. Children are a Vec in record order (repeated across merged files);
/// the same id twice is one member, two distinct same-named ATTRIBUTE children
/// (never emitted by one parser) is ambiguous → None, never first-wins.
fn enum_member(g: &RepoGraph, enum_id: NodeId, name: &str) -> Option<NodeId> {
    let mut hit: Option<NodeId> = None;
    for &child in g.nav.children_of.get(&enum_id)? {
        if g.nav.kind_by_id.get(&child) != Some(&node_kind::ATTRIBUTE)
            || g.nav.name_by_id.get(&child).map(String::as_str) != Some(name)
        {
            continue;
        }
        match hit {
            Some(existing) if existing == child => {}
            Some(_) => return None,
            None => hit = Some(child),
        }
    }
    hit
}

/// Search every class/struct's method map for a method named `name`.
/// Returns the NodeId iff exactly one class has it (avoids fabricating
/// edges when the same method name lives on multiple types). ENUM owners are
/// skipped: `class_methods` indexes them since LA.30a, and a route handler
/// binding must not turn ambiguous (or change target) because an enum happens
/// to own a same-named method — this pool stays CLASS / STRUCT only.
fn unique_global_method(g: &RepoGraph, name: &str) -> Option<NodeId> {
    let mut hit: Option<NodeId> = None;
    for (owner, methods) in &g.symbols.class_methods {
        if g.nav.kind_by_id.get(owner) == Some(&node_kind::ENUM) {
            continue;
        }
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

/// Walk parents to find the enclosing CLASS, STRUCT or ENUM — a type that owns
/// methods. Used to resolve self-method calls (Go `u.Save()`, TS `this.save()`,
/// Rust `self.weight()` inside `impl Tier`) to a sibling method on the same
/// type. The walk passes through intermediate parents, so a METHOD under an
/// ATTRIBUTE under an ENUM (a Java constant body) reaches the ENUM. The name
/// predates ENUM and is kept: other passes call it by this name.
fn enclosing_class_or_struct(nav: &CodeNav, start: NodeId) -> Option<NodeId> {
    let mut cur = start;
    loop {
        let parent = *nav.parent_of.get(&cur)?;
        let k = nav.kind_by_id.get(&parent).copied();
        if k == Some(node_kind::CLASS) || k == Some(node_kind::STRUCT) || k == Some(node_kind::ENUM)
        {
            return Some(parent);
        }
        cur = parent;
    }
}

// ============================================================================
// Receiver-type inference (A6.2a)
// ============================================================================

/// The field a call receiver names, one hop off the enclosing instance:
/// `this.svc` / `self.svc` / `svc` -> `Some("svc")`. Anything chained, called,
/// indexed, null-forgiving or conditional (`this.a.b`, `get().svc`, `a[0]`,
/// `svc?`, `svc!`) -> `None`: only a declared field of the enclosing type has a
/// type we know.
fn receiver_field(receiver: &str) -> Option<&str> {
    let r = receiver
        .strip_prefix("this.")
        .or_else(|| receiver.strip_prefix("self."))
        .unwrap_or(receiver);
    if r.is_empty() || r.contains(['.', '(', ')', '[', ']', ' ', '\t', '\n', '?', '!']) {
        return None;
    }
    Some(r)
}

/// A bare type name -> its node, through the same miss-only, ambiguity-safe
/// chain `resolve_refs` binds an INJECTS type with: the caller module's import
/// bindings, then its own symbols, then the repo-unique name
/// (`unique_global_function`; two distinct same-named types -> `None`).
fn resolve_type_name(g: &RepoGraph, from_module: NodeId, name: &str) -> Option<NodeId> {
    g.symbols
        .module_import_bindings
        .get(&from_module)
        .and_then(|b| b.get(name).copied())
        .or_else(|| {
            g.symbols
                .module_symbols
                .get(&from_module)
                .and_then(|s| s.get(name).copied())
        })
        .or_else(|| unique_global_function(g, name))
}

/// `<field>.m()` where `<field>` is a declared field of the innermost
/// enclosing CLASS / STRUCT / ENUM: bind `m` on the field's declared type.
/// Needs an exact declared type and an exact method name on it; an INTERFACE
/// type owns no `class_methods` entry, so interface-typed fields stay
/// unresolved (A6.6).
fn resolve_via_receiver_type(g: &RepoGraph, site: &CallSite, from_module: NodeId) -> Option<NodeId> {
    let (field, method) = match &site.qualifier {
        CallQualifier::Attribute { base, name } => (base.as_str(), name.as_str()),
        CallQualifier::ComplexReceiver { receiver, name } => {
            (receiver_field(receiver)?, name.as_str())
        }
        _ => return None,
    };
    if method.is_empty() {
        return None;
    }
    let owner = enclosing_class_or_struct(&g.nav, site.from)?;
    let type_name = g.nav.field_types.get(&owner)?.get(field)?;
    let type_id = resolve_type_name(g, from_module, type_name)?;
    g.symbols.class_methods.get(&type_id)?.get(method).copied()
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

    // ---- LA.30a: an ENUM owns its methods and members ----------------------

    /// One node per `(kind, qname, parent)`: the id, its nav record and its Node.
    struct Shape {
        nav: CodeNav,
        nodes: Vec<Node>,
    }

    impl Shape {
        fn new() -> Self {
            Shape { nav: CodeNav::default(), nodes: vec![] }
        }

        fn add(
            &mut self,
            kind: repo_graph_core::NodeKindId,
            qname: &str,
            parent: Option<NodeId>,
        ) -> NodeId {
            let r = repo();
            let id = NodeId::from_parts(GRAPH_TYPE, r, kind, qname);
            let name = qname.rsplit("::").next().unwrap_or(qname);
            self.nav.record(id, name, qname, kind, parent);
            self.nodes.push(Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] });
            id
        }

        fn file(
            self,
            imports: Vec<ImportStmt>,
            calls: Vec<CallSite>,
            refs: Vec<UnresolvedRef>,
        ) -> FileParse {
            FileParse {
                nodes: self.nodes,
                edges: vec![],
                imports,
                calls,
                refs,
                nav: self.nav,
                properties: HashSet::new(),
            }
        }
    }

    fn import_symbol(from: &str, module: &str, name: &str) -> ImportStmt {
        ImportStmt {
            from_module: from.to_string(),
            target: ImportTarget::Symbol {
                module: module.to_string(),
                name: name.to_string(),
                alias: None,
                level: 0,
            },
        }
    }

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute { base: base.to_string(), name: name.to_string() }
    }

    fn edges_of(g: &RepoGraph, category: EdgeCategoryId) -> Vec<(NodeId, NodeId)> {
        g.edges.iter().filter(|e| e.category == category).map(|e| (e.from, e.to)).collect()
    }

    /// `m2`: `enum E { RED; fn pick() }` — the imported-enum side of the
    /// attribute / USES tests.
    fn enum_module() -> (FileParse, NodeId, NodeId, NodeId) {
        let mut s = Shape::new();
        let m2 = s.add(node_kind::MODULE, "m2", None);
        let e = s.add(node_kind::ENUM, "m2::E", Some(m2));
        let red = s.add(node_kind::ATTRIBUTE, "m2::E::RED", Some(e));
        let pick = s.add(node_kind::METHOD, "m2::E::pick", Some(e));
        (s.file(vec![], vec![], vec![]), e, red, pick)
    }

    /// Rust `impl Tier { fn rank(&self) { self.weight() } }`: SelfMethod walks
    /// to the ENUM and binds its own method.
    #[test]
    fn enum_self_method_resolves_against_the_enum() {
        let mut s = Shape::new();
        let m = s.add(node_kind::MODULE, "m", None);
        let e = s.add(node_kind::ENUM, "m::E", Some(m));
        let a = s.add(node_kind::METHOD, "m::E::a", Some(e));
        let b = s.add(node_kind::METHOD, "m::E::b", Some(e));
        let site = CallSite { from: a, qualifier: CallQualifier::SelfMethod("b".to_string()) };
        let g = build_dotted(repo(), vec![s.file(vec![], vec![site], vec![])]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(a, b)]);
        assert!(g.unresolved_calls.is_empty());
    }

    /// Java constant body: a METHOD under an ATTRIBUTE under the ENUM. The walk
    /// passes through the ATTRIBUTE parent and stops at the ENUM.
    #[test]
    fn self_method_through_an_attribute_parent_reaches_the_enum() {
        let mut s = Shape::new();
        let m = s.add(node_kind::MODULE, "m", None);
        let e = s.add(node_kind::ENUM, "m::E", Some(m));
        let x = s.add(node_kind::ATTRIBUTE, "m::E::X", Some(e));
        let inner = s.add(node_kind::METHOD, "m::E::X::m", Some(x));
        let b = s.add(node_kind::METHOD, "m::E::b", Some(e));
        let site = CallSite { from: inner, qualifier: CallQualifier::SelfMethod("b".to_string()) };
        let g = build_dotted(repo(), vec![s.file(vec![], vec![site], vec![])]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(inner, b)]);
    }

    /// `import m2.E` + `E.pick()`: the bound base is an ENUM, which scopes
    /// methods through `class_methods` exactly like a CLASS.
    #[test]
    fn attribute_call_binds_enum_base() {
        let (enum_file, _, _, pick) = enum_module();
        let mut s = Shape::new();
        let m1 = s.add(node_kind::MODULE, "m1", None);
        let f = s.add(node_kind::FUNCTION, "m1::f", Some(m1));
        let caller = s.file(
            vec![import_symbol("m1", "m2", "E")],
            vec![CallSite { from: f, qualifier: attr("E", "pick") }],
            vec![],
        );
        let g = build_dotted(repo(), vec![enum_file, caller]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(f, pick)]);
        assert!(g.unresolved_calls.is_empty());
    }

    /// `E.RED` read as a USES ref binds the ENUM's ATTRIBUTE member.
    #[test]
    fn uses_ref_binds_enum_member() {
        let (enum_file, _, red, _) = enum_module();
        let mut s = Shape::new();
        let m1 = s.add(node_kind::MODULE, "m1", None);
        let f = s.add(node_kind::FUNCTION, "m1::f", Some(m1));
        let uses = UnresolvedRef {
            from: f,
            from_module: m1,
            qualifier: attr("E", "RED"),
            category: edge_category::USES,
        };
        let caller = s.file(vec![import_symbol("m1", "m2", "E")], vec![], vec![uses]);
        let g = build_dotted(repo(), vec![enum_file, caller]).unwrap();
        assert_eq!(edges_of(&g, edge_category::USES), vec![(f, red)]);
        assert!(g.unresolved_refs.is_empty(), "the member ref must bind");
    }

    /// The member lookup is ENUM-only: a CLASS base keeps HEAD's behaviour
    /// (methods only), so `C.X` against a class attribute stays unresolved.
    #[test]
    fn uses_ref_on_a_class_base_stays_unresolved() {
        let mut s2 = Shape::new();
        let m2 = s2.add(node_kind::MODULE, "m2", None);
        let c = s2.add(node_kind::CLASS, "m2::C", Some(m2));
        s2.add(node_kind::ATTRIBUTE, "m2::C::X", Some(c));
        let mut s = Shape::new();
        let m1 = s.add(node_kind::MODULE, "m1", None);
        let f = s.add(node_kind::FUNCTION, "m1::f", Some(m1));
        let uses = UnresolvedRef {
            from: f,
            from_module: m1,
            qualifier: attr("C", "X"),
            category: edge_category::USES,
        };
        let caller = s.file(vec![import_symbol("m1", "m2", "C")], vec![], vec![uses]);
        let g = build_dotted(repo(), vec![s2.file(vec![], vec![], vec![]), caller]).unwrap();
        assert!(edges_of(&g, edge_category::USES).is_empty());
        assert_eq!(g.unresolved_refs.len(), 1);
    }

    /// A call site `E.RED()` never binds a member: the member lookup is
    /// USES-only and `class_methods` indexes an ENUM's METHOD children only.
    #[test]
    fn calls_ref_never_binds_an_enum_member() {
        let (enum_file, _, _, _) = enum_module();
        let mut s = Shape::new();
        let m1 = s.add(node_kind::MODULE, "m1", None);
        let f = s.add(node_kind::FUNCTION, "m1::f", Some(m1));
        let caller = s.file(
            vec![import_symbol("m1", "m2", "E")],
            vec![CallSite { from: f, qualifier: attr("E", "RED") }],
            vec![],
        );
        let g = build_dotted(repo(), vec![enum_file, caller]).unwrap();
        assert!(edges_of(&g, edge_category::CALLS).is_empty());
        assert_eq!(g.unresolved_calls.len(), 1);
    }

    /// HANDLED_BY's global fallback keeps HEAD's CLASS / STRUCT pool: an ENUM
    /// owning a same-named `run` must not make the handler ambiguous.
    #[test]
    fn enum_methods_stay_out_of_the_handled_by_global_pool() {
        let mut s2 = Shape::new();
        let m2 = s2.add(node_kind::MODULE, "m2", None);
        let handlers = s2.add(node_kind::CLASS, "m2::Handlers", Some(m2));
        let class_run = s2.add(node_kind::METHOD, "m2::Handlers::run", Some(handlers));
        let mode = s2.add(node_kind::ENUM, "m2::Mode", Some(m2));
        let enum_run = s2.add(node_kind::METHOD, "m2::Mode::run", Some(mode));
        let mut s = Shape::new();
        let m1 = s.add(node_kind::MODULE, "m1", None);
        let route = s.add(node_kind::FUNCTION, "m1::routes", Some(m1));
        let handled = UnresolvedRef {
            from: route,
            from_module: m1,
            qualifier: attr("h", "run"),
            category: edge_category::HANDLED_BY,
        };
        let caller = s.file(vec![], vec![], vec![handled]);
        let g = build_dotted(repo(), vec![s2.file(vec![], vec![], vec![]), caller]).unwrap();
        // The ENUM's method is indexed (it owns it) but stays out of this pool.
        assert_eq!(g.symbols.class_methods[&mode].get("run").copied(), Some(enum_run));
        assert_eq!(edges_of(&g, edge_category::HANDLED_BY), vec![(route, class_run)]);
    }

    // ---- A6.2a: receiver-type inference -------------------------------------

    #[test]
    fn receiver_field_strips_this_and_rejects_chains() {
        assert_eq!(receiver_field("this.svc"), Some("svc"));
        assert_eq!(receiver_field("self.svc"), Some("svc"));
        assert_eq!(receiver_field("svc"), Some("svc"));
        assert_eq!(receiver_field("this.a.b"), None);
        assert_eq!(receiver_field("get().svc"), None);
        assert_eq!(receiver_field("items[0]"), None);
        assert_eq!(receiver_field("this.svc?"), None);
        assert_eq!(receiver_field("this."), None);
        assert_eq!(receiver_field(""), None);
    }

    /// `m2`: `class UserRepo { find() }` — the declared type of the field.
    fn repo_module() -> (FileParse, NodeId, NodeId) {
        let mut s = Shape::new();
        let m2 = s.add(node_kind::MODULE, "m2", None);
        let repo_cls = s.add(node_kind::CLASS, "m2::UserRepo", Some(m2));
        let find = s.add(node_kind::METHOD, "m2::UserRepo::find", Some(repo_cls));
        (s.file(vec![], vec![], vec![]), repo_cls, find)
    }

    /// `m1`: `class A { UserRepo repo; get() { <qualifier> } }`.
    fn caller_module(
        qualifier: CallQualifier,
        imports: Vec<ImportStmt>,
    ) -> (FileParse, NodeId, NodeId) {
        let mut s = Shape::new();
        let m1 = s.add(node_kind::MODULE, "m1", None);
        let a = s.add(node_kind::CLASS, "m1::A", Some(m1));
        let get = s.add(node_kind::METHOD, "m1::A::get", Some(a));
        s.nav.record_field_type(a, "repo", "UserRepo");
        (s.file(imports, vec![CallSite { from: get, qualifier }], vec![]), a, get)
    }

    #[test]
    fn field_typed_receiver_binds_to_declared_type_method() {
        let (repo_file, _, find) = repo_module();
        let (caller, _, get) = caller_module(attr("repo", "find"), vec![]);
        let g = build_dotted(repo(), vec![repo_file, caller]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(get, find)]);
        assert!(g.unresolved_calls.is_empty(), "the field-typed call must bind");
    }

    /// `this.repo.find()` reaches the pass as a ComplexReceiver.
    #[test]
    fn this_prefixed_complex_receiver_binds_through_the_field() {
        let (repo_file, _, find) = repo_module();
        let qualifier = CallQualifier::ComplexReceiver {
            receiver: "this.repo".to_string(),
            name: "find".to_string(),
        };
        let (caller, _, get) = caller_module(qualifier, vec![]);
        let g = build_dotted(repo(), vec![repo_file, caller]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(get, find)]);
    }

    /// `repo` is ALSO an import binding (a class `m3::repo` with its own
    /// `find`): the import binding resolves first and keeps its target.
    #[test]
    fn field_type_does_not_shadow_import_binding() {
        let (repo_file, _, typed_find) = repo_module();
        let mut s3 = Shape::new();
        let m3 = s3.add(node_kind::MODULE, "m3", None);
        let imported = s3.add(node_kind::CLASS, "m3::repo", Some(m3));
        let imported_find = s3.add(node_kind::METHOD, "m3::repo::find", Some(imported));
        let (caller, _, get) =
            caller_module(attr("repo", "find"), vec![import_symbol("m1", "m3", "repo")]);
        let g = build_dotted(
            repo(),
            vec![repo_file, s3.file(vec![], vec![], vec![]), caller],
        )
        .unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(get, imported_find)]);
        assert!(!edges_of(&g, edge_category::CALLS).contains(&(get, typed_find)));
    }

    /// The field belongs to its declaring type only: a sibling class B with no
    /// `repo` field calling `repo.find()` stays unresolved, and an unknown
    /// method on the declared type never binds.
    #[test]
    fn field_types_are_scoped_to_their_owner_and_need_an_exact_method() {
        let (repo_file, _, _) = repo_module();
        let (mut caller, _, get) = caller_module(attr("repo", "missing"), vec![]);
        let m1 = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "m1");
        let b = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::CLASS, "m1::B");
        let run = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::METHOD, "m1::B::run");
        caller.nav.record(b, "B", "m1::B", node_kind::CLASS, Some(m1));
        caller.nav.record(run, "run", "m1::B::run", node_kind::METHOD, Some(b));
        caller.calls.push(CallSite { from: run, qualifier: attr("repo", "find") });
        let g = build_dotted(repo(), vec![repo_file, caller]).unwrap();
        assert!(edges_of(&g, edge_category::CALLS).is_empty());
        assert_eq!(g.unresolved_calls.len(), 2);
        assert_eq!(g.unresolved_calls[0].from, get);
    }

    /// Two distinct `UserRepo` types in the repo: the type name is ambiguous,
    /// so the call stays unresolved rather than binding either one.
    #[test]
    fn ambiguous_declared_type_stays_unresolved() {
        let (repo_file, _, _) = repo_module();
        let mut s4 = Shape::new();
        let m4 = s4.add(node_kind::MODULE, "m4", None);
        let twin = s4.add(node_kind::CLASS, "m4::UserRepo", Some(m4));
        s4.add(node_kind::METHOD, "m4::UserRepo::find", Some(twin));
        let (caller, _, _) = caller_module(attr("repo", "find"), vec![]);
        let g = build_dotted(repo(), vec![repo_file, s4.file(vec![], vec![], vec![]), caller])
            .unwrap();
        assert!(edges_of(&g, edge_category::CALLS).is_empty());
        assert_eq!(g.unresolved_calls.len(), 1);
    }

    /// An INTERFACE-typed field owns no `class_methods`, so it stays
    /// unresolved until interface dispatch (A6.6) lands.
    #[test]
    fn interface_typed_field_stays_unresolved() {
        let mut s2 = Shape::new();
        let m2 = s2.add(node_kind::MODULE, "m2", None);
        let iface = s2.add(node_kind::INTERFACE, "m2::UserRepo", Some(m2));
        s2.add(node_kind::METHOD, "m2::UserRepo::find", Some(iface));
        let (caller, _, _) = caller_module(attr("repo", "find"), vec![]);
        let g = build_dotted(repo(), vec![s2.file(vec![], vec![], vec![]), caller]).unwrap();
        assert!(edges_of(&g, edge_category::CALLS).is_empty());
        assert_eq!(g.unresolved_calls.len(), 1);
    }

    /// Two files contributing fields to one owner (a C# `partial class`) keep
    /// both after `merge_nav`.
    #[test]
    fn field_types_merge_per_owner_across_files() {
        let (repo_file, _, find) = repo_module();
        let (caller, a, get) = caller_module(attr("other", "find"), vec![]);
        let mut part = Shape::new();
        part.nav.record_field_type(a, "other", "UserRepo");
        let g = build_dotted(repo(), vec![repo_file, caller, part.file(vec![], vec![], vec![])])
            .unwrap();
        assert_eq!(g.nav.field_types[&a].len(), 2);
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(get, find)]);
    }
}
