//! Rust crate-path call resolution (LA.1a): `crate::`, `self::`, `super::`,
//! `Self::`, child-module, same-module type and workspace-crate paths
//! (`repo_graph_engine::service_map(..)`) become CALLS edges the generic
//! symbol-table walker cannot reach.
//!
//! The Rust parser already extracts the whole path: `a::b::c()` arrives as
//! `CallQualifier::Attribute { base: "a::b", name: "c" }`. What the parser
//! cannot know is what `crate`, `super`, a sibling crate's name or a child
//! module mean: those are cross-file facts. This module resolves them in the
//! graph crate through `resolve_calls`' `extra_hook` seam, so it runs only
//! after the generic pass misses: every edge that resolved before resolves
//! identically, non-Rust graphs never see it, and Rust graphs only gain edges.
//!
//! The engine feeds it one [`RustCrate`] per Cargo package the walk found. A
//! module no package covers (loose `.rs` files) takes the nearest `lib` /
//! `main` file up its directory as its crate root, else the repo root.
//!
//! LA.3 adds the Rust item-coverage passes that ride on the same index: a
//! `self.m()` whose `impl` sits in another file than its type, the inline-mod
//! Bare pre-pass (`resolve_scoped_bare`, innermost `mod` first), the
//! enum-variant USES refs (`resolve_leftover_refs`) and the `[rust-items]`
//! marker.
//!
//! Every lookup is by key, and every candidate list is sorted by qname before
//! a tie-break, so no winner is ever picked by iterating a `HashMap`.

use std::cell::Cell;
use std::collections::HashMap;

use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, cell_type, edge_category, node_kind,
};
use repo_graph_core::{CellPayload, NodeId, NodeKindId};

use crate::calls::{position_file, push_edge};
use crate::types::RepoGraph;

/// One Cargo package's crate roots, as the engine found them in the walk.
/// Outside this crate, build it from `Default` plus field assignment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RustCrate {
    /// Path identifier: the package name with `-` -> `_`
    /// (`repo-graph-engine` -> `repo_graph_engine`).
    pub name: String,
    /// Package dir as a qname prefix (`engine`; `""` for a package at the repo root).
    pub dir: String,
    /// Module qname of the library root (`engine::src::lib`), when the package has one.
    pub lib_root: Option<String>,
    /// Every other crate root it owns, as module qnames: `src/main.rs`,
    /// `src/bin/*.rs`, `src/bin/*/main.rs`, `tests/*.rs`, `examples/*.rs`,
    /// `benches/*.rs`.
    pub other_roots: Vec<String>,
}

/// How a path's first segment found its start node. The `[rust-paths]`
/// marker counts each; they sum to the number of path calls examined.
#[derive(Clone, Copy)]
enum Rule {
    Crate,
    SelfMod,
    Super,
    SelfType,
    Import,
    Child,
    Type,
    CrateName,
}

/// Where a path stands after some of its segments.
enum Pos {
    /// A file MODULE, or an inline-mod PACKAGE (LA.3).
    Scope(NodeId),
    /// The crate root of loose `.rs` files that have no `lib` / `main`: the
    /// repo root, whose child modules sit at the empty prefix.
    Virtual,
    /// A STRUCT / ENUM / CLASS / INTERFACE.
    Type(NodeId),
    /// `Self` in a method whose `impl` lives in another file than its type
    /// (the parser parents it to the MODULE, qname `<module>::<Type>::<m>`).
    TypeName(String),
}

/// A crate root: a module qname, or the loose-file repo root.
enum Root {
    Module(String),
    Virtual,
}

/// One package, with its roots indexed by the prefix their children live at.
struct Package {
    name: String,
    dir: String,
    lib_root: Option<String>,
    /// Children prefix -> the package's crate roots whose child modules live
    /// there (`engine::src` -> [`engine::src::lib`, `engine::src::main`]), sorted.
    roots_by_prefix: HashMap<String, Vec<String>>,
}

#[derive(Default)]
struct Stats {
    by_rule: [Cell<usize>; 8],
    resolved: Cell<usize>,
    unique_in_crate: Cell<usize>,
    receiver_skipped: Cell<usize>,
}

impl Stats {
    fn bump(c: &Cell<usize>) {
        c.set(c.get() + 1);
    }
}

/// A METHOD of an `impl` written outside its type's file: (qname, id, package).
type ElsewhereMethod = (String, NodeId, Option<usize>);

/// Owned lookup tables over one Rust graph, built once after the symbol
/// table. The hook receives the graph by `&` on every call, so only derived
/// indexes live here.
pub(crate) struct RustIndex {
    packages: Vec<Package>,
    /// Every package's crate root qname -> the package index.
    roots: HashMap<String, usize>,
    /// Crate root qname -> (top-level def name -> defs across the file
    /// modules of that crate, sorted by qname, deduped). Feeds the
    /// unique-in-crate fallback at a crate root.
    items: HashMap<String, HashMap<String, Vec<NodeId>>>,
    /// (type simple name, member) -> METHODs whose nav parent is a MODULE or
    /// an inline-mod PACKAGE (an `impl` outside its type's container), as
    /// (qname, id, package), sorted by qname.
    impl_elsewhere: HashMap<(String, String), Vec<ElsewhereMethod>>,
    /// Node id -> index into `g.nodes`, for the caller's CODE cell.
    node_pos: HashMap<NodeId, usize>,
    crates: usize,
    stats: Stats,
}

impl RustIndex {
    pub(crate) fn build(g: &RepoGraph, crates: &[RustCrate]) -> Self {
        let mut sorted: Vec<&RustCrate> = crates.iter().collect();
        sorted.sort_by(|a, b| (&a.dir, &a.name).cmp(&(&b.dir, &b.name)));
        let mut idx = RustIndex {
            packages: Vec::with_capacity(sorted.len()),
            roots: HashMap::new(),
            items: HashMap::new(),
            impl_elsewhere: HashMap::new(),
            node_pos: g.nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect(),
            crates: crates.len(),
            stats: Stats::default(),
        };
        for (i, c) in sorted.iter().enumerate() {
            for r in c.lib_root.iter().chain(&c.other_roots) {
                idx.roots.entry(r.clone()).or_insert(i);
            }
            idx.packages.push(Package {
                name: c.name.clone(),
                dir: c.dir.clone(),
                lib_root: c.lib_root.clone(),
                roots_by_prefix: HashMap::new(),
            });
        }
        // Children prefixes need `roots` complete (a root's prefix is its parent).
        for (i, c) in sorted.iter().enumerate() {
            let mut by_prefix: HashMap<String, Vec<String>> = HashMap::new();
            for r in c.lib_root.iter().chain(&c.other_roots) {
                by_prefix
                    .entry(idx.children_prefix(r))
                    .or_default()
                    .push(r.clone());
            }
            for list in by_prefix.values_mut() {
                list.sort();
                list.dedup();
            }
            idx.packages[i].roots_by_prefix = by_prefix;
        }

        let mut modules: Vec<(&String, NodeId)> = g
            .symbols
            .module_by_qname
            .iter()
            .map(|(q, id)| (q, *id))
            .collect();
        modules.sort_by(|a, b| a.0.cmp(b.0));
        let mut items: HashMap<String, HashMap<String, Vec<(String, NodeId)>>> = HashMap::new();
        for (q, id) in modules {
            let Some(Root::Module(root)) = idx.crate_root(g, q) else {
                continue;
            };
            let Some(children) = g.nav.children_of.get(&id) else {
                continue;
            };
            for child in children {
                let is_def = g
                    .nav
                    .kind_by_id
                    .get(child)
                    .is_some_and(|k| *k == node_kind::FUNCTION || is_type_kind(*k));
                if let (true, Some(name), Some(cq)) = (
                    is_def,
                    g.nav.name_by_id.get(child),
                    g.nav.qname_by_id.get(child),
                ) {
                    let per_root = items.entry(root.clone()).or_default();
                    per_root
                        .entry(name.clone())
                        .or_default()
                        .push((cq.clone(), *child));
                }
            }
        }
        idx.items = items
            .into_iter()
            .map(|(root, names)| {
                let names = names
                    .into_iter()
                    .map(|(n, defs)| (n, sorted_ids(defs)))
                    .collect();
                (root, names)
            })
            .collect();

        for (id, kind) in &g.nav.kind_by_id {
            if *kind != node_kind::METHOD {
                continue;
            }
            let Some(parent) = g.nav.parent_of.get(id) else {
                continue;
            };
            // A method whose `impl` names a type its container does not
            // define: parented to the file MODULE, or (LA.3) to the inline-mod
            // PACKAGE it sits in (`mod tests { impl Foo { .. } }`).
            if !g
                .nav
                .kind_by_id
                .get(parent)
                .is_some_and(|k| is_scope_kind(*k))
            {
                continue;
            }
            let (Some(pq), Some(q)) = (g.nav.qname_by_id.get(parent), g.nav.qname_by_id.get(id))
            else {
                continue;
            };
            let Some((ty, member)) = q
                .strip_prefix(pq.as_str())
                .and_then(|rest| rest.strip_prefix("::"))
                .and_then(|rest| rest.split_once("::"))
            else {
                continue;
            };
            if member.contains("::") {
                continue;
            }
            let pkg = idx.covering(pq);
            idx.impl_elsewhere
                .entry((ty.to_string(), member.to_string()))
                .or_default()
                .push((q.clone(), *id, pkg));
        }
        for list in idx.impl_elsewhere.values_mut() {
            list.sort_by(|a, b| a.0.cmp(&b.0));
            list.dedup_by(|a, b| a.1 == b.1);
        }
        idx
    }

    /// The `extra_hook` body: resolve one Attribute call site as a Rust path,
    /// or (LA.3) one `self.m()` the generic owner walk missed. `None` for
    /// every other qualifier shape.
    pub(crate) fn resolve_call(&self, g: &RepoGraph, site: &CallSite) -> Option<NodeId> {
        let (base, name) = match &site.qualifier {
            CallQualifier::Attribute { base, name } => (base, name),
            CallQualifier::SelfMethod(name) => return self.resolve_self_method(g, site.from, name),
            _ => return None,
        };
        if name.is_empty() {
            return None;
        }
        let segs = split_path(base)?;
        let scope = enclosing(&g.nav, site.from, true)?;
        let file_module = enclosing(&g.nav, site.from, false)?;
        let (start, rule) = self.start(g, site.from, scope, file_module, &segs[0])?;
        // `x.m()` and `x::m()` reach here in one shape. A lone base found by
        // an import / child-module / type / crate-name rule may be a local
        // variable that shares the name; the caller's own source decides.
        let lone_ident = segs.len() == 1
            && matches!(
                rule,
                Rule::Import | Rule::Child | Rule::Type | Rule::CrateName
            );
        if lone_ident && self.method_shaped(g, site.from, &segs[0], name) {
            Stats::bump(&self.stats.receiver_skipped);
            return None;
        }
        Stats::bump(&self.stats.by_rule[rule as usize]);
        let mut used_fallback = false;
        let pos = self.walk(g, start, &segs[1..], &mut used_fallback)?;
        let pos = self.settle_type_name(g, pos, file_module);
        let caller_pkg = g
            .nav
            .qname_by_id
            .get(&file_module)
            .and_then(|q| self.covering(q));
        let hit = self.final_name(g, &pos, name, caller_pkg, &mut used_fallback)?;
        Stats::bump(&self.stats.resolved);
        if used_fallback {
            Stats::bump(&self.stats.unique_in_crate);
        }
        Some(hit)
    }

    /// LA.1a fired_on marker, once per `build_rust` that examined a path call:
    /// `[rust-paths] path calls resolved R/P (crate=.. self=.. super=.. Self=..
    /// import=.. child=.. type=.. crate_name=.. unique_in_crate=..) crates=N`,
    /// plus ` receiver_skipped=K` when the lone-base guard dropped any site.
    pub(crate) fn report(&self) {
        let n = |r: Rule| self.stats.by_rule[r as usize].get();
        let examined: usize = self.stats.by_rule.iter().map(Cell::get).sum();
        if examined == 0 {
            return;
        }
        let skipped = self.stats.receiver_skipped.get();
        let tail = if skipped > 0 {
            format!(" receiver_skipped={skipped}")
        } else {
            String::new()
        };
        eprintln!(
            "[rust-paths] path calls resolved {}/{examined} (crate={} self={} super={} Self={} \
             import={} child={} type={} crate_name={} unique_in_crate={}) crates={}{tail}",
            self.stats.resolved.get(),
            n(Rule::Crate),
            n(Rule::SelfMod),
            n(Rule::Super),
            n(Rule::SelfType),
            n(Rule::Import),
            n(Rule::Child),
            n(Rule::Type),
            n(Rule::CrateName),
            self.stats.unique_in_crate.get(),
            self.crates,
        );
    }

    // ---- LA.3: self calls, inline-mod scoping, enum variants --------------

    /// `self.name()` the generic pass missed. For a METHOD parented to its
    /// type that is a member written in another file's `impl` (or a trait
    /// default); for a METHOD parented to a MODULE / PACKAGE (its `impl`
    /// names a type defined in another file, e.g. `impl MergedGraph` in
    /// `blast.rs`) the type is the one crate-local STRUCT / ENUM / CLASS of
    /// that name, else the other-file impls of that name.
    fn resolve_self_method(&self, g: &RepoGraph, from: NodeId, name: &str) -> Option<NodeId> {
        if name.is_empty() {
            return None;
        }
        let file_module = enclosing(&g.nav, from, false)?;
        let pos = self.settle_type_name(g, self_type(&g.nav, from)?, file_module);
        let caller_pkg = g
            .nav
            .qname_by_id
            .get(&file_module)
            .and_then(|q| self.covering(q));
        let mut unused = false;
        self.final_name(g, &pos, name, caller_pkg, &mut unused)
    }

    /// `Pos::TypeName(ty)` -> `Pos::Type` when the caller's crate defines
    /// exactly one top-level type named `ty`; anything else unchanged.
    fn settle_type_name(&self, g: &RepoGraph, pos: Pos, file_module: NodeId) -> Pos {
        let Pos::TypeName(ty) = &pos else {
            return pos;
        };
        let root = g
            .nav
            .qname_by_id
            .get(&file_module)
            .and_then(|q| self.crate_root(g, q));
        let Some(Root::Module(root)) = root else {
            return pos;
        };
        let types: Vec<NodeId> = self
            .items
            .get(&root)
            .and_then(|names| names.get(ty.as_str()))
            .into_iter()
            .flatten()
            .copied()
            .filter(|id| g.nav.kind_by_id.get(id).is_some_and(|k| is_type_kind(*k)))
            .collect();
        match types.as_slice() {
            [only] => Pos::Type(*only),
            _ => pos,
        }
    }

    /// The inline-mod pre-pass for one `Bare(name)` call site: the first
    /// callable `name` defined directly in a PACKAGE ancestor of the caller,
    /// innermost first, stopping at the file MODULE. Rust scoping is
    /// innermost-first, while the generic Bare order checks the FILE module
    /// before any PACKAGE, so without this `fn helper` inside `mod endpoint`
    /// would lose to a file-level `fn helper`. `None` when the caller is in no
    /// inline mod or no enclosing mod defines `name`: the site then goes to
    /// the generic pass (the `use super::*` case).
    pub(crate) fn resolve_scoped_bare(&self, g: &RepoGraph, site: &CallSite) -> Option<NodeId> {
        let CallQualifier::Bare(name) = &site.qualifier else {
            return None;
        };
        let mut cur = site.from;
        loop {
            let parent = *g.nav.parent_of.get(&cur)?;
            match g.nav.kind_by_id.get(&parent) {
                Some(k) if *k == node_kind::MODULE => return None,
                Some(k) if *k == node_kind::PACKAGE => {
                    let hit = g
                        .symbols
                        .module_symbols
                        .get(&parent)
                        .and_then(|s| s.get(name))
                        .filter(|id| g.nav.kind_by_id.get(*id).is_some_and(|k| is_callable(*k)));
                    if let Some(id) = hit {
                        return Some(*id);
                    }
                }
                _ => {}
            }
            cur = parent;
        }
    }

    /// Bind the enum-variant USES refs `resolve_refs` left unresolved (a
    /// same-file, path-qualified or `Self::` variant; an imported enum's
    /// variant already bound there through the module's import bindings).
    /// Drains `g.unresolved_refs`; misses go back in their original order,
    /// every other ref untouched.
    pub(crate) fn resolve_leftover_refs(&self, g: &mut RepoGraph) {
        let pending = std::mem::take(&mut g.unresolved_refs);
        for r in pending {
            let hit = match &r.qualifier {
                CallQualifier::Attribute { base, name } if r.category == edge_category::USES => {
                    self.resolve_variant(g, r.from, base, name)
                }
                _ => None,
            };
            match hit {
                Some(to) => push_edge(g, r.from, to, edge_category::USES),
                None => g.unresolved_refs.push(r),
            }
        }
    }

    /// The ATTRIBUTE variant `name` of the enum the path `base` names, from
    /// the caller `from`'s scope: the same first-segment rules and walk as a
    /// path call, ending on an ENUM instead of a callable.
    fn resolve_variant(
        &self,
        g: &RepoGraph,
        from: NodeId,
        base: &str,
        name: &str,
    ) -> Option<NodeId> {
        let segs = split_path(base)?;
        let scope = enclosing(&g.nav, from, true)?;
        let file_module = enclosing(&g.nav, from, false)?;
        let (start, _) = self.start(g, from, scope, file_module, &segs[0])?;
        let mut unused = false;
        let pos = self.walk(g, start, &segs[1..], &mut unused)?;
        match self.settle_type_name(g, pos, file_module) {
            Pos::Type(ty) if g.nav.kind_by_id.get(&ty) == Some(&node_kind::ENUM) => {
                enum_variant(&g.nav, ty, name)
            }
            _ => None,
        }
    }

    /// LA.3 fired_on marker, once per `build_rust`:
    /// `[rust-items] inline_mods=N fns_in_inline_mods=F enum_variants=V
    /// variant_uses=R/T enum_self_calls=S mod_scoped_calls=M`. Counted off the
    /// built graph, so cache-served files count too: N = PACKAGE nodes in a
    /// `.rs` file, F = FUNCTION / METHOD nodes under one, V = ATTRIBUTE
    /// children of an ENUM, R/T = USES edges onto a variant / those plus the
    /// USES refs still unresolved, S = CALLS from a method to a method of its
    /// own enum, M = the inline-mod pre-pass's binds (`mod_scoped`).
    pub(crate) fn report_items(&self, g: &RepoGraph, mod_scoped: usize) {
        let kind = |id: &NodeId| g.nav.kind_by_id.get(id).copied();
        let parent_kind = |id: &NodeId| g.nav.parent_of.get(id).and_then(kind);
        let inline_mods = g
            .nav
            .kind_by_id
            .iter()
            .filter(|(id, k)| {
                **k == node_kind::PACKAGE
                    && self
                        .node_pos
                        .get(*id)
                        .and_then(|i| g.nodes.get(*i))
                        .and_then(position_file)
                        .is_some_and(|f| f.ends_with(".rs"))
            })
            .count();
        let fns_in_mods = g
            .nav
            .kind_by_id
            .iter()
            .filter(|(id, k)| {
                (**k == node_kind::FUNCTION || **k == node_kind::METHOD)
                    && enclosing(&g.nav, **id, true)
                        .is_some_and(|s| kind(&s) == Some(node_kind::PACKAGE))
            })
            .count();
        let is_variant = |id: &NodeId| {
            kind(id) == Some(node_kind::ATTRIBUTE) && parent_kind(id) == Some(node_kind::ENUM)
        };
        let variants = g.nav.kind_by_id.keys().filter(|id| is_variant(id)).count();
        let uses_bound = g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::USES && is_variant(&e.to))
            .count();
        let uses_left = g
            .unresolved_refs
            .iter()
            .filter(|r| r.category == edge_category::USES)
            .count();
        let enum_self = g
            .edges
            .iter()
            .filter(|e| {
                e.category == edge_category::CALLS && kind(&e.to) == Some(node_kind::METHOD)
            })
            .filter(|e| {
                let Some(owner) = g
                    .nav
                    .parent_of
                    .get(&e.to)
                    .filter(|p| kind(p) == Some(node_kind::ENUM))
                else {
                    return false;
                };
                kind(&e.from) == Some(node_kind::METHOD)
                    && match self_type(&g.nav, e.from) {
                        Some(Pos::Type(t)) => t == *owner,
                        Some(Pos::TypeName(n)) => g.nav.name_by_id.get(owner) == Some(&n),
                        _ => false,
                    }
            })
            .count();
        eprintln!(
            "[rust-items] inline_mods={inline_mods} fns_in_inline_mods={fns_in_mods} \
             enum_variants={variants} variant_uses={uses_bound}/{} enum_self_calls={enum_self} \
             mod_scoped_calls={mod_scoped}",
            uses_bound + uses_left
        );
    }

    // ---- first segment ------------------------------------------------------

    fn start(
        &self,
        g: &RepoGraph,
        from: NodeId,
        scope: NodeId,
        file_module: NodeId,
        seg: &str,
    ) -> Option<(Pos, Rule)> {
        match seg {
            "crate" => {
                let q = g.nav.qname_by_id.get(&file_module)?;
                Some((self.root_pos(g, q)?, Rule::Crate))
            }
            "self" => Some((Pos::Scope(scope), Rule::SelfMod)),
            "super" => Some((self.parent_pos(g, scope)?, Rule::Super)),
            "Self" => Some((self_type(&g.nav, from)?, Rule::SelfType)),
            _ => {
                if let Some(p) = self.binding_pos(g, scope, seg).or_else(|| {
                    (scope != file_module)
                        .then(|| self.binding_pos(g, file_module, seg))
                        .flatten()
                }) {
                    return Some((p, Rule::Import));
                }
                // Innermost scope first, then (LA.3) each enclosing scope out
                // to the file module: an inline `mod tests` reaches its parent's
                // modules and types, the `use super::*` every test mod opens
                // with. Top-level code has no enclosing scope, so this is
                // LA.1a's lookup unchanged there.
                let mut cur = Some(scope);
                while let Some(s) = cur {
                    if let Some(p) = self.child_pos(g, &Pos::Scope(s), seg) {
                        return Some((p, Rule::Child));
                    }
                    if let Some(p) = type_pos(g, s, seg) {
                        return Some((p, Rule::Type));
                    }
                    cur = (s != file_module)
                        .then(|| g.nav.parent_of.get(&s))
                        .flatten()
                        .and_then(|up| enclosing(&g.nav, *up, true));
                }
                let q = g.nav.qname_by_id.get(&file_module)?;
                Some((self.crate_name_pos(g, q, seg)?, Rule::CrateName))
            }
        }
    }

    /// A workspace crate named `seg`, as its library root. Two packages may
    /// share a name (two bench fixtures both call theirs `acme-app`): the one
    /// whose dir shares the most `::` segments with the caller wins; a tie is
    /// unresolved.
    fn crate_name_pos(&self, g: &RepoGraph, caller: &str, seg: &str) -> Option<Pos> {
        let mut best: Option<(usize, &str)> = None;
        let mut tied = false;
        for p in &self.packages {
            let Some(lib) = p.lib_root.as_deref().filter(|_| p.name == seg) else {
                continue;
            };
            let score = common_segments(&p.dir, caller);
            match best {
                Some((s, _)) if s > score => {}
                Some((s, _)) if s == score => tied = true,
                _ => {
                    best = Some((score, lib));
                    tied = false;
                }
            }
        }
        let (_, lib) = best.filter(|_| !tied)?;
        g.symbols.module_by_qname.get(lib).map(|id| Pos::Scope(*id))
    }

    // ---- remaining segments -------------------------------------------------

    fn walk(
        &self,
        g: &RepoGraph,
        mut pos: Pos,
        rest: &[String],
        fallback: &mut bool,
    ) -> Option<Pos> {
        for seg in rest {
            pos = match pos {
                Pos::Scope(id) if seg == "super" => self.parent_pos(g, id)?,
                Pos::Scope(id) => self
                    .child_pos(g, &pos, seg)
                    .or_else(|| type_pos(g, id, seg))
                    .or_else(|| self.binding_pos(g, id, seg))
                    .or_else(|| {
                        let hit = self.unique_in_crate(g, id, seg, true).map(Pos::Type);
                        *fallback |= hit.is_some();
                        hit
                    })?,
                Pos::Virtual => self.child_pos(g, &pos, seg)?,
                Pos::Type(_) | Pos::TypeName(_) => return None,
            };
        }
        Some(pos)
    }

    fn final_name(
        &self,
        g: &RepoGraph,
        pos: &Pos,
        name: &str,
        caller_pkg: Option<usize>,
        fallback: &mut bool,
    ) -> Option<NodeId> {
        match pos {
            Pos::Scope(id) => {
                let callable =
                    |n: &NodeId| g.nav.kind_by_id.get(n).is_some_and(|k| is_callable(*k));
                let direct = g
                    .symbols
                    .module_symbols
                    .get(id)
                    .and_then(|s| s.get(name))
                    .copied();
                let bound = || {
                    g.symbols
                        .module_import_bindings
                        .get(id)
                        .and_then(|b| b.get(name))
                        .copied()
                };
                direct
                    .filter(callable)
                    .or_else(|| bound().filter(callable))
                    .or_else(|| {
                        let hit = self.unique_in_crate(g, *id, name, false);
                        *fallback |= hit.is_some();
                        hit
                    })
            }
            Pos::Virtual => None,
            Pos::Type(ty) => g
                .symbols
                .class_methods
                .get(ty)
                .and_then(|m| m.get(name))
                .or_else(|| {
                    g.symbols
                        .interface_methods
                        .get(ty)
                        .and_then(|m| m.get(name))
                })
                .copied()
                .or_else(|| {
                    let ty_name = g.nav.name_by_id.get(ty)?;
                    let module = enclosing(&g.nav, *ty, false)?;
                    let pkg = g
                        .nav
                        .qname_by_id
                        .get(&module)
                        .and_then(|q| self.covering(q));
                    self.elsewhere(ty_name, name, pkg)
                }),
            Pos::TypeName(ty_name) => self.elsewhere(ty_name, name, caller_pkg),
        }
    }

    /// The METHOD `member` of an `impl <ty_name>` written outside the type's
    /// own file, when exactly one exists in package `pkg`.
    fn elsewhere(&self, ty_name: &str, member: &str, pkg: Option<usize>) -> Option<NodeId> {
        let list = self
            .impl_elsewhere
            .get(&(ty_name.to_string(), member.to_string()))?;
        let mut hits = list.iter().filter(|(_, _, p)| *p == pkg);
        let (_, id, _) = hits.next()?;
        hits.next().is_none().then_some(*id)
    }

    /// The crate-root fallback: exactly one top-level def named `name` across
    /// the crate whose root is `scope` (a type when `want_type`). It is what
    /// lets `acme_core::service_map` bind while the root's `pub use api::..`
    /// re-export is unparsed. Only at a crate root; ambiguity -> `None`.
    fn unique_in_crate(
        &self,
        g: &RepoGraph,
        scope: NodeId,
        name: &str,
        want_type: bool,
    ) -> Option<NodeId> {
        let q = g.nav.qname_by_id.get(&scope)?;
        if g.nav.kind_by_id.get(&scope) != Some(&node_kind::MODULE) || !self.is_root(q) {
            return None;
        }
        let [id] = self.items.get(q)?.get(name)?.as_slice() else {
            return None;
        };
        let k = *g.nav.kind_by_id.get(id)?;
        let fits = if want_type {
            is_type_kind(k)
        } else {
            is_callable(k)
        };
        fits.then_some(*id)
    }

    // ---- module tree ----------------------------------------------------------

    /// A module or inline-mod PACKAGE child named `seg` of `pos`.
    fn child_pos(&self, g: &RepoGraph, pos: &Pos, seg: &str) -> Option<Pos> {
        let (prefix, scope) = match pos {
            Pos::Scope(id) => {
                let kind = g.nav.kind_by_id.get(id)?;
                let prefix = if *kind == node_kind::MODULE {
                    Some(self.children_prefix(g.nav.qname_by_id.get(id)?))
                } else {
                    None
                };
                (prefix, Some(*id))
            }
            Pos::Virtual => (Some(String::new()), None),
            Pos::Type(_) | Pos::TypeName(_) => return None,
        };
        if let Some(prefix) = prefix {
            let q = join(&prefix, seg);
            let file_child = g
                .symbols
                .module_by_qname
                .get(&q)
                .or_else(|| g.symbols.module_by_qname.get(&join(&q, "mod")));
            if let Some(id) = file_child {
                return Some(Pos::Scope(*id));
            }
        }
        let children = g.nav.children_of.get(&scope?)?;
        children
            .iter()
            .filter(|c| {
                g.nav.kind_by_id.get(c) == Some(&node_kind::PACKAGE)
                    && g.nav.name_by_id.get(c).is_some_and(|n| n == seg)
            })
            .filter_map(|c| g.nav.qname_by_id.get(c).map(|q| (q, *c)))
            .min_by(|a, b| a.0.cmp(b.0))
            .map(|(_, id)| Pos::Scope(id))
    }

    /// `super` of a scope: an inline PACKAGE's nav parent scope; a file
    /// module's parent is the module whose children prefix is its own module
    /// path minus the last segment (`<p>::mod`, `<p>`, or the crate root). A
    /// crate root has none.
    fn parent_pos(&self, g: &RepoGraph, scope: NodeId) -> Option<Pos> {
        if g.nav.kind_by_id.get(&scope) == Some(&node_kind::PACKAGE) {
            let up = *g.nav.parent_of.get(&scope)?;
            return enclosing(&g.nav, up, true).map(Pos::Scope);
        }
        let q = g.nav.qname_by_id.get(&scope)?;
        if self.is_root(q) {
            return None;
        }
        let want = parent_path(module_path(q));
        match self.crate_root(g, q) {
            Some(Root::Module(r)) if r != *q && self.children_prefix(&r) == want => {
                return g.symbols.module_by_qname.get(&r).map(|id| Pos::Scope(*id));
            }
            Some(Root::Virtual) if want.is_empty() => return Some(Pos::Virtual),
            _ => {}
        }
        [join(want, "mod"), want.to_string()]
            .into_iter()
            .filter(|c| !c.is_empty() && self.children_prefix(c) == want)
            .find_map(|c| g.symbols.module_by_qname.get(&c).map(|id| Pos::Scope(*id)))
    }

    fn root_pos(&self, g: &RepoGraph, q: &str) -> Option<Pos> {
        match self.crate_root(g, q)? {
            Root::Module(r) => g.symbols.module_by_qname.get(&r).map(|id| Pos::Scope(*id)),
            Root::Virtual => Some(Pos::Virtual),
        }
    }

    /// The crate root of file module `q`. Inside a package: walk up `q`'s
    /// directories to the first one some crate root of that package keeps
    /// its children in (the library root wins a shared directory; two other
    /// roots there is ambiguous -> `None`). Outside every package: the
    /// nearest `lib` / `main` module up the directories, else the repo root.
    fn crate_root(&self, g: &RepoGraph, q: &str) -> Option<Root> {
        if self.is_root(q) {
            return Some(Root::Module(q.to_string()));
        }
        let mut dir = parent_path(module_path(q));
        match self.covering(q) {
            Some(pi) => {
                let pkg = &self.packages[pi];
                loop {
                    if let Some(list) = pkg.roots_by_prefix.get(dir) {
                        if let Some(lib) = pkg.lib_root.as_ref().filter(|l| list.contains(l)) {
                            return Some(Root::Module(lib.clone()));
                        }
                        return match list.as_slice() {
                            [only] => Some(Root::Module(only.clone())),
                            _ => None,
                        };
                    }
                    if dir.len() <= pkg.dir.len() {
                        return None;
                    }
                    dir = parent_path(dir);
                }
            }
            None => loop {
                for leaf in ["lib", "main"] {
                    let c = join(dir, leaf);
                    if g.symbols.module_by_qname.contains_key(&c) {
                        return Some(Root::Module(c));
                    }
                }
                if dir.is_empty() {
                    return Some(Root::Virtual);
                }
                dir = parent_path(dir);
            },
        }
    }

    /// The package whose dir is the longest prefix of `q`.
    fn covering(&self, q: &str) -> Option<usize> {
        let mut best: Option<(usize, usize)> = None;
        for (i, p) in self.packages.iter().enumerate() {
            let inside = p.dir.is_empty()
                || q.strip_prefix(p.dir.as_str())
                    .is_some_and(|r| r.starts_with("::"));
            if inside && best.is_none_or(|(len, _)| p.dir.len() > len) {
                best = Some((p.dir.len(), i));
            }
        }
        best.map(|(_, i)| i)
    }

    fn is_root(&self, q: &str) -> bool {
        self.roots.contains_key(q)
            || (self.covering(q).is_none() && matches!(last_seg(q), "lib" | "main"))
    }

    /// Where `q`'s child modules live: beside it for a crate root or a
    /// `mod.rs`, else in the directory named after it (Rust 2018 `a.rs` +
    /// `a/b.rs`).
    fn children_prefix(&self, q: &str) -> String {
        if last_seg(q) == "mod" || self.is_root(q) {
            parent_path(q).to_string()
        } else {
            q.to_string()
        }
    }

    fn binding_pos(&self, g: &RepoGraph, scope: NodeId, seg: &str) -> Option<Pos> {
        let id = *g.symbols.module_import_bindings.get(&scope)?.get(seg)?;
        let kind = *g.nav.kind_by_id.get(&id)?;
        if kind == node_kind::MODULE || kind == node_kind::PACKAGE {
            Some(Pos::Scope(id))
        } else if is_type_kind(kind) {
            Some(Pos::Type(id))
        } else {
            None
        }
    }

    /// True when the caller's own source writes `base.name` and never
    /// `base::name`: a method call on a value that shares a module / type /
    /// crate name, not a path. No CODE cell -> false (cannot tell).
    fn method_shaped(&self, g: &RepoGraph, from: NodeId, base: &str, name: &str) -> bool {
        let code = self
            .node_pos
            .get(&from)
            .and_then(|i| g.nodes.get(*i))
            .filter(|n| n.id == from)
            .and_then(|n| {
                n.cells
                    .iter()
                    .find_map(|c| match (&c.payload, c.kind == cell_type::CODE) {
                        (CellPayload::Text(t), true) => Some(t.as_str()),
                        _ => None,
                    })
            });
        code.is_some_and(|code| {
            joined_by(code, base, ".", name) && !joined_by(code, base, "::", name)
        })
    }
}

// ---- free helpers ----------------------------------------------------------

/// `base` split into path segments: a leading `::` dropped, generic arguments
/// (`Vec::<u32>`, `HashMap::<K, a::V>`) removed, `r#` stripped. `None` for a
/// qualified-self base (`<T as Trait>`) or anything not made of identifiers.
fn split_path(base: &str) -> Option<Vec<String>> {
    let base = base.trim();
    let base = base.strip_prefix("::").unwrap_or(base);
    if base.is_empty() || base.starts_with('<') {
        return None;
    }
    let mut plain = String::with_capacity(base.len());
    let mut depth = 0usize;
    let mut prev = ' ';
    for ch in base.chars() {
        match ch {
            '<' => depth += 1,
            '>' if prev != '-' => depth = depth.checked_sub(1)?,
            _ if depth == 0 => plain.push(ch),
            _ => {}
        }
        prev = ch;
    }
    if depth != 0 {
        return None;
    }
    let segs: Vec<String> = plain
        .split("::")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.strip_prefix("r#").unwrap_or(s).to_string())
        .collect();
    let ident = |s: &String| s.chars().all(|c| c.is_alphanumeric() || c == '_');
    (!segs.is_empty() && segs.iter().all(ident)).then_some(segs)
}

/// Whether `code` holds `base <sep> name` as whole identifiers, whitespace
/// allowed around `sep`. Indices only ever move past whole matches or ASCII
/// bytes, so every slice lands on a char boundary.
fn joined_by(code: &str, base: &str, sep: &str, name: &str) -> bool {
    let bytes = code.as_bytes();
    let ident_byte = |b: u8| b.is_ascii_alphanumeric() || b == b'_' || b >= 0x80;
    let skip_ws = |mut i: usize| {
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        i
    };
    code.match_indices(base).any(|(at, _)| {
        if at > 0 && ident_byte(bytes[at - 1]) {
            return false;
        }
        let i = skip_ws(at + base.len());
        if !code[i..].starts_with(sep) {
            return false;
        }
        let j = skip_ws(i + sep.len());
        code[j..].starts_with(name) && !bytes.get(j + name.len()).is_some_and(|b| ident_byte(*b))
    })
}

/// Nearest ancestor-or-self MODULE, or MODULE / PACKAGE when `or_package`.
fn enclosing(nav: &CodeNav, mut id: NodeId, or_package: bool) -> Option<NodeId> {
    loop {
        let kind = nav.kind_by_id.get(&id);
        if kind == Some(&node_kind::MODULE) || (or_package && kind == Some(&node_kind::PACKAGE)) {
            return Some(id);
        }
        id = *nav.parent_of.get(&id)?;
    }
}

/// `Self` of the caller: its enclosing type, or, for a method parented to a
/// MODULE (its `impl` lives in another file), the type segment of its qname.
fn self_type(nav: &CodeNav, from: NodeId) -> Option<Pos> {
    let mut cur = from;
    loop {
        let parent = *nav.parent_of.get(&cur)?;
        let kind = *nav.kind_by_id.get(&parent)?;
        if is_type_kind(kind) {
            return Some(Pos::Type(parent));
        }
        if kind == node_kind::MODULE || kind == node_kind::PACKAGE {
            if nav.kind_by_id.get(&cur) != Some(&node_kind::METHOD) {
                return None;
            }
            let q = nav.qname_by_id.get(&cur)?;
            let ty = parent_path(q).rsplit("::").next()?;
            return (!ty.is_empty()).then(|| Pos::TypeName(ty.to_string()));
        }
        cur = parent;
    }
}

/// A type defined directly in `scope`.
fn type_pos(g: &RepoGraph, scope: NodeId, seg: &str) -> Option<Pos> {
    let id = *g.symbols.module_symbols.get(&scope)?.get(seg)?;
    is_type_kind(*g.nav.kind_by_id.get(&id)?).then_some(Pos::Type(id))
}

/// A scope items are declared in: a file MODULE or an inline-mod PACKAGE.
fn is_scope_kind(k: NodeKindId) -> bool {
    k == node_kind::MODULE || k == node_kind::PACKAGE
}

/// The ATTRIBUTE child of `enum_id` named `name` (a variant). The same id
/// twice (children repeat across merged files) is one variant; two distinct
/// same-named children is ambiguous -> `None`, never first-wins.
fn enum_variant(nav: &CodeNav, enum_id: NodeId, name: &str) -> Option<NodeId> {
    let mut hit: Option<NodeId> = None;
    for &child in nav.children_of.get(&enum_id)? {
        if nav.kind_by_id.get(&child) != Some(&node_kind::ATTRIBUTE)
            || nav.name_by_id.get(&child).map(String::as_str) != Some(name)
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

fn is_type_kind(k: NodeKindId) -> bool {
    k == node_kind::STRUCT
        || k == node_kind::ENUM
        || k == node_kind::CLASS
        || k == node_kind::INTERFACE
}

/// What a module-level path call may land on: a fn, or a tuple-struct constructor.
fn is_callable(k: NodeKindId) -> bool {
    k == node_kind::FUNCTION || k == node_kind::STRUCT || k == node_kind::CLASS
}

fn sorted_ids(mut defs: Vec<(String, NodeId)>) -> Vec<NodeId> {
    defs.sort_by(|a, b| a.0.cmp(&b.0));
    defs.dedup_by(|a, b| a.1 == b.1);
    defs.into_iter().map(|(_, id)| id).collect()
}

fn join(prefix: &str, seg: &str) -> String {
    if prefix.is_empty() {
        seg.to_string()
    } else {
        format!("{prefix}::{seg}")
    }
}

fn last_seg(q: &str) -> &str {
    q.rsplit("::").next().unwrap_or(q)
}

/// `q` minus its last segment; `""` for a single segment.
fn parent_path(q: &str) -> &str {
    q.rsplit_once("::").map_or("", |(p, _)| p)
}

/// A file module's module path: `x::y::mod` -> `x::y`, anything else as is.
fn module_path(q: &str) -> &str {
    if q == "mod" {
        ""
    } else {
        q.strip_suffix("::mod").unwrap_or(q)
    }
}

/// Leading `::` segments two qnames share.
fn common_segments(a: &str, b: &str) -> usize {
    if a.is_empty() {
        return 0;
    }
    a.split("::")
        .zip(b.split("::"))
        .take_while(|(x, y)| x == y)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_path_drops_generics_and_rejects_qualified_self() {
        let s = |b: &str| split_path(b).map(|v| v.join("|"));
        assert_eq!(s("crate::api").as_deref(), Some("crate|api"));
        assert_eq!(s("::std::fs").as_deref(), Some("std|fs"));
        assert_eq!(s("Vec::<u32>").as_deref(), Some("Vec"));
        assert_eq!(
            s("HashMap::<K, a::V>::inner").as_deref(),
            Some("HashMap|inner")
        );
        assert_eq!(s("Box::<dyn Fn() -> u32>").as_deref(), Some("Box"));
        assert_eq!(s("r#type::x").as_deref(), Some("type|x"));
        assert_eq!(s("<T as Trait>"), None);
        assert_eq!(s("a.b"), None);
        assert_eq!(s(""), None);
    }

    #[test]
    fn joined_by_needs_whole_identifiers() {
        let code = "fn f() { cfg.get(); api :: helper(); xapi::other(); é.x(); }";
        assert!(joined_by(code, "cfg", ".", "get"));
        assert!(!joined_by(code, "cfg", "::", "get"));
        assert!(joined_by(code, "api", "::", "helper"));
        assert!(!joined_by(code, "api", "::", "other"), "xapi is not api");
        assert!(!joined_by(code, "api", "::", "help"), "helper is not help");
        assert!(joined_by(code, "é", ".", "x"));
    }

    #[test]
    fn module_path_helpers() {
        assert_eq!(module_path("a::b::mod"), "a::b");
        assert_eq!(module_path("a::b"), "a::b");
        assert_eq!(parent_path("a::b"), "a");
        assert_eq!(parent_path("a"), "");
        assert_eq!(common_segments("ws1::core", "ws1::app::src::main"), 1);
        assert_eq!(common_segments("", "x"), 0);
    }
}
