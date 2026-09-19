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
//! LA.1b resolves the `use` trees the parser emits as raw Rust paths
//! ([`resolve_imports_rust`]): workspace-crate uses, `pub use` re-exports
//! (chains followed), aliases, globs and `use`s inside fn bodies bind, in the
//! scope that holds them. A file MODULE's or inline PACKAGE's bindings go in
//! the persisted `module_import_bindings`; a fn's and every glob stay in the
//! build-time [`RustBindings`] the call passes read.
//!
//! LA.35b binds the typed receivers the generic receiver pass (A6.2a /
//! LA.35a) misses ([`RustIndex::resolve_call`]'s `ComplexReceiver` arm): a
//! method from an `impl` in another file than its type, `self.f.m()` inside
//! such an `impl`, and a receiver type only Rust scoping names (a fn-body
//! `use`, a glob, an inline mod, the one crate-local type of that name).
//!
//! Every lookup is by key, and every candidate list is sorted by qname before
//! a tie-break, so no winner is ever picked by iterating a `HashMap`.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{
    CallQualifier, CallSite, CodeNav, ImportStmt, ImportTarget, cell_type, edge_category,
    node_kind, recv_stats,
};
use repo_graph_core::{CellPayload, NodeId, NodeKindId};

use crate::calls::{
    position_file, push_edge, receiver_type, unique_global_function, unique_global_module,
};
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

/// How the hook bound a typed receiver (LA.35b); the `[rust-recv]` marker
/// counts each, one per bound call, first matching rule wins.
#[derive(Clone, Copy)]
enum Recv {
    /// `self.f.m()` in an `impl` written in another file than its type: the
    /// owner came from the caller's qname.
    CrossFileSelfField,
    /// The receiver's type is an ENUM.
    EnumMember,
    /// The method sits in an `impl` in another file than its type.
    ImplElsewhere,
    /// A type the generic pass cannot name (a fn-body `use`, a glob, an
    /// inline mod, the one crate-local type of that name), method on the type.
    ScopedType,
}

#[derive(Default)]
struct Stats {
    by_rule: [Cell<usize>; 8],
    resolved: Cell<usize>,
    unique_in_crate: Cell<usize>,
    receiver_skipped: Cell<usize>,
    recv: [Cell<usize>; 4],
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
    /// (LA.3) one `self.m()` the generic owner walk missed, (LA.1b) one Bare
    /// call through the file module's `use ..::*` globs, the last scope Rust
    /// consults, or (LA.35b) one method call on a typed value
    /// ([`Self::resolve_typed_receiver`]). `None` for every other shape.
    pub(crate) fn resolve_call(
        &self,
        g: &RepoGraph,
        site: &CallSite,
        rb: &RustBindings,
    ) -> Option<NodeId> {
        let (base, name) = match &site.qualifier {
            CallQualifier::Attribute { base, name } => (base, name),
            CallQualifier::SelfMethod(name) => {
                return self.resolve_self_method(g, rb, site.from, name);
            }
            CallQualifier::Bare(name) => {
                let file_module = enclosing(&g.nav, site.from, false)?;
                return self.glob_member(g, rb, file_module, name, is_callable);
            }
            CallQualifier::ComplexReceiver { name, .. } => {
                return self.resolve_typed_receiver(g, rb, site, name);
            }
            _ => return None,
        };
        if name.is_empty() {
            return None;
        }
        let segs = split_path(base)?;
        let scope = enclosing(&g.nav, site.from, true)?;
        let file_module = enclosing(&g.nav, site.from, false)?;
        let (start, rule) = self.start(g, rb, site.from, scope, file_module, &segs[0])?;
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
        let pos = self.walk(g, rb, start, &segs[1..], &mut used_fallback)?;
        let pos = self.settle_type_name(g, pos, file_module);
        let caller_pkg = g
            .nav
            .qname_by_id
            .get(&file_module)
            .and_then(|q| self.covering(q));
        let hit = self.final_name(g, rb, &pos, name, caller_pkg, &mut used_fallback)?;
        Stats::bump(&self.stats.resolved);
        if used_fallback {
            Stats::bump(&self.stats.unique_in_crate);
        }
        Some(hit)
    }

    /// The fired_on markers of one `build_rust`: LA.1a's path line, then
    /// LA.35b's [`Self::report_recv`] line.
    pub(crate) fn report(&self) {
        self.report_paths();
        self.report_recv();
    }

    /// LA.1a fired_on marker, once per `build_rust` that examined a path call:
    /// `[rust-paths] path calls resolved R/P (crate=.. self=.. super=.. Self=..
    /// import=.. child=.. type=.. crate_name=.. unique_in_crate=..) crates=N`,
    /// plus ` receiver_skipped=K` when the lone-base guard dropped any site.
    fn report_paths(&self) {
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

    /// LA.35b fired_on marker, once per `build_rust` whose hook bound a typed
    /// receiver: `[rust-recv] typed receivers via hook: impl_elsewhere=A
    /// cross_file_self_field=B enum_member=C`, plus ` scoped_type=D` when any
    /// bind needed only Rust scoping. One count per bound call ([`Recv`]).
    fn report_recv(&self) {
        let n = |r: Recv| self.stats.recv[r as usize].get();
        if self.stats.recv.iter().all(|c| c.get() == 0) {
            return;
        }
        let scoped = n(Recv::ScopedType);
        let tail = if scoped > 0 {
            format!(" scoped_type={scoped}")
        } else {
            String::new()
        };
        eprintln!(
            "[rust-recv] typed receivers via hook: impl_elsewhere={} cross_file_self_field={} \
             enum_member={}{tail}",
            n(Recv::ImplElsewhere),
            n(Recv::CrossFileSelfField),
            n(Recv::EnumMember),
        );
    }

    // ---- LA.35b: typed receivers --------------------------------------------

    /// `x.m()` / `self.f.m()` the generic receiver pass missed. The receiver's
    /// type name is [`receiver_type`]'s (a local, else a field of the
    /// enclosing type), else, for `self.f` in an `impl` written in another
    /// file than its type, field `f` of the type the caller's qname names
    /// ([`Self::cross_file_field_type`]). The name becomes a type the Rust way
    /// ([`Self::type_in_scope`]) and the method is found on it by
    /// [`Self::final_name`]: its own methods, a trait's, then (crate-scoped,
    /// unique) the `impl`s in other files. A receiver no rule types binds
    /// nothing: a shadowing local of unknown type stays unresolved.
    fn resolve_typed_receiver(
        &self,
        g: &RepoGraph,
        rb: &RustBindings,
        site: &CallSite,
        name: &str,
    ) -> Option<NodeId> {
        if name.is_empty() {
            return None;
        }
        let (pos, cross_file) = match receiver_type(g, site) {
            Some(ty) => (self.type_in_scope(g, rb, site.from, ty)?, false),
            None => (self.cross_file_field_type(g, rb, site)?, true),
        };
        let file_module = enclosing(&g.nav, site.from, false)?;
        let caller_pkg = g
            .nav
            .qname_by_id
            .get(&file_module)
            .and_then(|q| self.covering(q));
        let mut unused = false;
        let hit = self.final_name(g, rb, &pos, name, caller_pkg, &mut unused)?;
        let kind = |id: &NodeId| g.nav.kind_by_id.get(id).copied();
        let rule = if cross_file {
            Recv::CrossFileSelfField
        } else if matches!(&pos, Pos::Type(t) if kind(t) == Some(node_kind::ENUM)) {
            Recv::EnumMember
        } else if g
            .nav
            .parent_of
            .get(&hit)
            .and_then(kind)
            .is_some_and(is_scope_kind)
        {
            Recv::ImplElsewhere
        } else {
            Recv::ScopedType
        };
        Stats::bump(&self.stats.recv[rule as usize]);
        recv_stats::record();
        Some(hit)
    }

    /// The type a type name written in `from`'s scope names: a `use` (the
    /// fn's own first), a type or glob-imported type in the enclosing scopes,
    /// innermost first; else the one top-level type of that name in `from`'s
    /// crate. No such type in the crate -> `Pos::TypeName` (an `impl` in the
    /// crate may still name it); two or more -> `None`. Never a same-named
    /// type of another crate the scope does not import.
    fn type_in_scope(
        &self,
        g: &RepoGraph,
        rb: &RustBindings,
        from: NodeId,
        ty: &str,
    ) -> Option<Pos> {
        let scope = enclosing(&g.nav, from, true)?;
        let file_module = enclosing(&g.nav, from, false)?;
        if let Some((Pos::Type(id), _)) = self.start(g, rb, from, scope, file_module, ty) {
            return Some(Pos::Type(id));
        }
        match self.crate_types(g, file_module, ty).as_slice() {
            [only] => Some(Pos::Type(*only)),
            [] => Some(Pos::TypeName(ty.to_string())),
            _ => None,
        }
    }

    /// `self.f` (or a local bound to `self.f`) in a method whose `impl` lives
    /// in another file than its type: the owner is the one crate-local STRUCT
    /// / ENUM named by the caller's qname (`<module>::<Type>::<m>`, LA.3's
    /// `Self` rule), and `f`'s declared type is resolved from the owner's
    /// scope, where the field was written. Ambiguity -> `None`.
    fn cross_file_field_type(
        &self,
        g: &RepoGraph,
        rb: &RustBindings,
        site: &CallSite,
    ) -> Option<Pos> {
        let field = self_field(g, site)?;
        let Pos::TypeName(owner_name) = self_type(&g.nav, site.from)? else {
            return None;
        };
        let file_module = enclosing(&g.nav, site.from, false)?;
        let owners: Vec<NodeId> = self
            .crate_types(g, file_module, &owner_name)
            .into_iter()
            .filter(|id| {
                g.nav
                    .kind_by_id
                    .get(id)
                    .is_some_and(|k| *k == node_kind::STRUCT || *k == node_kind::ENUM)
            })
            .collect();
        let [owner] = owners.as_slice() else {
            return None;
        };
        let ty = g
            .nav
            .field_types
            .get(owner)?
            .get(field)
            .filter(|t| !t.is_empty())?;
        self.type_in_scope(g, rb, *owner, ty)
    }

    /// The top-level types named `ty` in the crate of `file_module`, sorted
    /// by qname. Empty when the crate is unknown.
    fn crate_types(&self, g: &RepoGraph, file_module: NodeId, ty: &str) -> Vec<NodeId> {
        let root = g
            .nav
            .qname_by_id
            .get(&file_module)
            .and_then(|q| self.crate_root(g, q));
        let Some(Root::Module(root)) = root else {
            return Vec::new();
        };
        self.items
            .get(&root)
            .and_then(|names| names.get(ty))
            .into_iter()
            .flatten()
            .copied()
            .filter(|id| g.nav.kind_by_id.get(id).is_some_and(|k| is_type_kind(*k)))
            .collect()
    }

    // ---- LA.3: self calls, inline-mod scoping, enum variants --------------

    /// `self.name()` the generic pass missed. For a METHOD parented to its
    /// type that is a member written in another file's `impl` (or a trait
    /// default); for a METHOD parented to a MODULE / PACKAGE (its `impl`
    /// names a type defined in another file, e.g. `impl MergedGraph` in
    /// `blast.rs`) the type is the one crate-local STRUCT / ENUM / CLASS of
    /// that name, else the other-file impls of that name.
    fn resolve_self_method(
        &self,
        g: &RepoGraph,
        rb: &RustBindings,
        from: NodeId,
        name: &str,
    ) -> Option<NodeId> {
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
        self.final_name(g, rb, &pos, name, caller_pkg, &mut unused)
    }

    /// `Pos::TypeName(ty)` -> `Pos::Type` when the caller's crate defines
    /// exactly one top-level type named `ty`; anything else unchanged.
    fn settle_type_name(&self, g: &RepoGraph, pos: Pos, file_module: NodeId) -> Pos {
        let Pos::TypeName(ty) = &pos else {
            return pos;
        };
        match self.crate_types(g, file_module, ty).as_slice() {
            [only] => Pos::Type(*only),
            _ => pos,
        }
    }

    /// The scoped pre-pass for one `Bare(name)` call site, innermost scope
    /// first: (LA.1b) the caller fn's own `use`s, then its `use ..::*` globs;
    /// then each PACKAGE ancestor out to the file MODULE (LA.3): its own
    /// callable `name`, its `use` binding, (LA.1b) its globs. Rust scoping is
    /// innermost-first, while the generic Bare order checks the FILE module
    /// before any PACKAGE, so without this `fn helper` inside `mod endpoint`
    /// would lose to a file-level `fn helper`, and a fn-body
    /// `use lib::generate_many as gm` to a file-level `fn gm`. A site whose
    /// caller has no fn-scoped `use` and sits in no inline mod finds nothing
    /// here and keeps the generic order exactly. The bool is true for a hit
    /// on an inline mod's own fn, the LA.3 rule the `[rust-items]` marker's
    /// `mod_scoped_calls` counts.
    pub(crate) fn resolve_scoped_bare(
        &self,
        g: &RepoGraph,
        site: &CallSite,
        rb: &RustBindings,
    ) -> Option<(NodeId, bool)> {
        let CallQualifier::Bare(name) = &site.qualifier else {
            return None;
        };
        let callable = |id: &NodeId| g.nav.kind_by_id.get(id).is_some_and(|k| is_callable(*k));
        let fn_hit = rb
            .fn_scoped
            .get(&site.from)
            .and_then(|m| m.get(name))
            .copied()
            .filter(callable)
            .or_else(|| self.glob_member(g, rb, site.from, name, is_callable));
        if let Some(id) = fn_hit {
            return Some((id, false));
        }
        let mut cur = site.from;
        loop {
            let parent = *g.nav.parent_of.get(&cur)?;
            match g.nav.kind_by_id.get(&parent) {
                Some(k) if *k == node_kind::MODULE => return None,
                Some(k) if *k == node_kind::PACKAGE => {
                    let own = g
                        .symbols
                        .module_symbols
                        .get(&parent)
                        .and_then(|s| s.get(name))
                        .copied()
                        .filter(callable);
                    if let Some(id) = own {
                        return Some((id, true));
                    }
                    let used = rb
                        .bound(g, parent, name)
                        .filter(callable)
                        .or_else(|| self.glob_member(g, rb, parent, name, is_callable));
                    if let Some(id) = used {
                        return Some((id, false));
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
    pub(crate) fn resolve_leftover_refs(&self, g: &mut RepoGraph, rb: &RustBindings) {
        let pending = std::mem::take(&mut g.unresolved_refs);
        for r in pending {
            let hit = match &r.qualifier {
                CallQualifier::Attribute { base, name } if r.category == edge_category::USES => {
                    self.resolve_variant(g, rb, r.from, base, name)
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
        rb: &RustBindings,
        from: NodeId,
        base: &str,
        name: &str,
    ) -> Option<NodeId> {
        let segs = split_path(base)?;
        let scope = enclosing(&g.nav, from, true)?;
        let file_module = enclosing(&g.nav, from, false)?;
        let (start, _) = self.start(g, rb, from, scope, file_module, &segs[0])?;
        let mut unused = false;
        let pos = self.walk(g, rb, start, &segs[1..], &mut unused)?;
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

    /// The first segment of a path written in `from` (a fn or method, or for
    /// a `use` the scope that holds it), whose enclosing MODULE / PACKAGE is
    /// `scope` and file module `file_module`.
    fn start(
        &self,
        g: &RepoGraph,
        sc: &dyn UseScopes,
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
                let chain = scope_chain(&g.nav, scope, file_module);
                // A `use` binding, innermost scope first: (LA.1b) the fn's
                // own body, then each scope out to the file module.
                let fn_scope = (from != scope).then_some(from);
                for s in fn_scope.iter().chain(&chain) {
                    if let Some(p) = self.binding_pos(g, sc, *s, seg) {
                        return Some((p, Rule::Import));
                    }
                }
                // Innermost scope first, then (LA.3) each enclosing scope out
                // to the file module: an inline `mod tests` reaches its parent's
                // modules and types, the `use super::*` every test mod opens
                // with. Top-level code has no enclosing scope, so this is
                // LA.1a's lookup unchanged there.
                for s in &chain {
                    if let Some(p) = self.child_pos(g, &Pos::Scope(*s), seg) {
                        return Some((p, Rule::Child));
                    }
                    if let Some(p) = type_pos(g, *s, seg) {
                        return Some((p, Rule::Type));
                    }
                }
                // A glob import (LA.1b), innermost first: shadowed by every
                // item and explicit `use` above, above the extern prelude.
                for s in fn_scope.iter().chain(&chain) {
                    let hit =
                        self.glob_member(g, sc, *s, seg, |k| is_scope_kind(k) || is_type_kind(k));
                    if let Some(p) = hit.and_then(|id| scope_or_type(g, id)) {
                        return Some((p, Rule::Import));
                    }
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
        sc: &dyn UseScopes,
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
                    .or_else(|| self.binding_pos(g, sc, id, seg))
                    .or_else(|| {
                        self.glob_member(g, sc, id, seg, |k| is_scope_kind(k) || is_type_kind(k))
                            .and_then(|hit| scope_or_type(g, hit))
                    })
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
        sc: &dyn UseScopes,
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
                direct
                    .filter(callable)
                    .or_else(|| {
                        let hit = sc.bound(g, *id, name).filter(callable);
                        hit.inspect(|_| sc.note_hop())
                    })
                    .or_else(|| self.glob_member(g, sc, *id, name, is_callable))
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

    /// A module / type that a `use` in `scope` binds as `seg`.
    fn binding_pos(
        &self,
        g: &RepoGraph,
        sc: &dyn UseScopes,
        scope: NodeId,
        seg: &str,
    ) -> Option<Pos> {
        let pos = scope_or_type(g, sc.bound(g, scope, seg)?)?;
        sc.note_hop();
        Some(pos)
    }

    /// `name` through the `use ..::*` globs of `scope` (LA.1b): every scope
    /// reachable over glob edges, breadth-first, each once, at most
    /// [`MAX_USE_HOPS`] globs deep. A reached scope contributes its own item,
    /// its explicit `use` binding or its child module named `name`, and only
    /// when it has none do its own globs count (Rust: a glob is shadowed by
    /// every explicit name). Hits `want` rejects are skipped. Exactly one
    /// distinct hit binds; two is ambiguous -> `None`, never first-wins.
    fn glob_member(
        &self,
        g: &RepoGraph,
        sc: &dyn UseScopes,
        scope: NodeId,
        name: &str,
        want: fn(NodeKindId) -> bool,
    ) -> Option<NodeId> {
        let mut frontier: Vec<NodeId> = sc.globs(scope).to_vec();
        if frontier.is_empty() {
            return None;
        }
        let mut seen: HashSet<NodeId> = HashSet::from([scope]);
        let mut hit: Option<NodeId> = None;
        for _ in 0..MAX_USE_HOPS {
            let mut next = Vec::new();
            for t in frontier {
                if !seen.insert(t) {
                    continue;
                }
                match (self.own_member(g, sc, t, name, want), hit) {
                    (Some(id), Some(h)) if id != h => return None,
                    (Some(id), _) => hit = Some(id),
                    (None, _) => next.extend_from_slice(sc.globs(t)),
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        hit.inspect(|_| sc.note_hop())
    }

    /// What scope `t` itself names `name`, of a kind `want` admits: an item
    /// it defines, an explicit `use` binding in it, or a child module /
    /// inline mod, in that order.
    fn own_member(
        &self,
        g: &RepoGraph,
        sc: &dyn UseScopes,
        t: NodeId,
        name: &str,
        want: fn(NodeKindId) -> bool,
    ) -> Option<NodeId> {
        let fits = |id: &NodeId| {
            g.nav
                .kind_by_id
                .get(id)
                .is_some_and(|k| is_bindable(*k) && want(*k))
        };
        g.symbols
            .module_symbols
            .get(&t)
            .and_then(|s| s.get(name))
            .copied()
            .filter(fits)
            .or_else(|| sc.bound(g, t, name).filter(fits))
            .or_else(|| match self.child_pos(g, &Pos::Scope(t), name)? {
                Pos::Scope(id) => Some(id).filter(fits),
                _ => None,
            })
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

// ---- LA.1b: `use` trees ------------------------------------------------------

/// The longest chain a `use` lookup follows: a re-export that needs more
/// than this many other `use`s to resolve, or a name more than this many
/// globs away, stays unresolved.
const MAX_USE_HOPS: usize = 8;

/// What [`resolve_imports_rust`] binds outside the persisted symbol table:
/// the `use`s inside fn bodies, and every glob. `module_import_bindings` is
/// written to the `.gmap` per module and two fns of one file may bind one name
/// to two targets, so a fn's bindings live here, owned by the call passes of
/// one `build_rust` and never stored.
#[derive(Default)]
pub(crate) struct RustBindings {
    /// fn / METHOD node -> (bound name -> target) for the `use`s in its body.
    fn_scoped: HashMap<NodeId, HashMap<String, NodeId>>,
    /// Scope (file MODULE, inline PACKAGE, fn) -> the scopes its `use ..::*`
    /// globs name, sorted by qname.
    globs: HashMap<NodeId, Vec<NodeId>>,
}

/// Where a lookup reads `use` bindings: the finished tables at call time
/// ([`RustBindings`] beside the graph's module bindings), or the previous
/// round's while [`resolve_imports_rust`] iterates ([`Round`]).
trait UseScopes {
    /// What an explicit `use` in `scope` binds `name` to.
    fn bound(&self, g: &RepoGraph, scope: NodeId, name: &str) -> Option<NodeId>;
    /// The scopes `scope`'s globs name.
    fn globs(&self, scope: NodeId) -> &[NodeId];
    /// A lookup went through a binding another `use` made (`reexport_hops`).
    fn note_hop(&self) {}
}

impl UseScopes for RustBindings {
    fn bound(&self, g: &RepoGraph, scope: NodeId, name: &str) -> Option<NodeId> {
        self.fn_scoped
            .get(&scope)
            .or_else(|| g.symbols.module_import_bindings.get(&scope))
            .and_then(|m| m.get(name))
            .copied()
    }

    fn globs(&self, scope: NodeId) -> &[NodeId] {
        self.globs.get(&scope).map_or(&[], Vec::as_slice)
    }
}

/// One round's binding tables, over every scope kind.
#[derive(Default, PartialEq)]
struct UseTables {
    bound: HashMap<NodeId, HashMap<String, NodeId>>,
    globs: HashMap<NodeId, Vec<NodeId>>,
}

impl UseTables {
    /// The tables one round's results make. Two `use`s of one scope binding
    /// one name to different targets (cfg-gated twins) bind it to neither.
    fn collect(g: &RepoGraph, leaves: &[UseLeaf<'_>], results: &[Option<UseHit>]) -> Self {
        let mut t = UseTables::default();
        let mut ambiguous: Vec<(NodeId, &str)> = Vec::new();
        for (leaf, hit) in leaves.iter().zip(results) {
            let Some(target) = hit.and_then(|h| h.target) else {
                continue;
            };
            if leaf.glob {
                t.globs.entry(leaf.scope).or_default().push(target);
                continue;
            }
            let Some(name) = leaf.bound else { continue };
            let slot = t.bound.entry(leaf.scope).or_default();
            match slot.get(name) {
                Some(prev) if *prev != target => ambiguous.push((leaf.scope, name)),
                _ => {
                    slot.insert(name.to_string(), target);
                }
            }
        }
        for (scope, name) in ambiguous {
            if let Some(m) = t.bound.get_mut(&scope) {
                m.remove(name);
            }
        }
        let qname = |id: &NodeId| g.nav.qname_by_id.get(id).map_or("", String::as_str);
        for list in t.globs.values_mut() {
            list.sort_by(|a, b| qname(a).cmp(qname(b)));
            list.dedup();
        }
        t
    }
}

/// [`UseScopes`] over the previous round's tables while one leaf resolves:
/// the name that leaf binds never resolves through itself (`use log::log;`
/// names the `log` crate), and every binding it goes through is counted.
struct Round<'a> {
    tables: &'a UseTables,
    skip: Option<(NodeId, &'a str)>,
    hops: Cell<usize>,
}

impl UseScopes for Round<'_> {
    fn bound(&self, _g: &RepoGraph, scope: NodeId, name: &str) -> Option<NodeId> {
        if self.skip == Some((scope, name)) {
            return None;
        }
        self.tables.bound.get(&scope)?.get(name).copied()
    }

    fn globs(&self, scope: NodeId) -> &[NodeId] {
        self.tables.globs.get(&scope).map_or(&[], Vec::as_slice)
    }

    fn note_hop(&self) {
        Stats::bump(&self.hops);
    }
}

/// One leaf of a `use` tree, placed in its scope.
struct UseLeaf<'a> {
    /// What holds the `use`: a file MODULE, an inline PACKAGE, or a fn / METHOD.
    scope: NodeId,
    /// The path resolved to a module or type (a Symbol's `module`, a
    /// Module's `path`); `None` when it is not made of identifiers.
    path: Option<Vec<String>>,
    /// `use path::member` names `member` in `path`; `None` binds `path` itself.
    member: Option<&'a str>,
    /// The name bound: the alias, else the last segment. `None` for a glob
    /// and for `as _`.
    bound: Option<&'a str>,
    glob: bool,
    alias: bool,
    /// A `crate::` / `super::` Symbol: HEAD resolved these, so a miss in a
    /// loose file keeps HEAD's tail fallback. Never an external crate's path.
    tail_fallback: bool,
}

/// A leaf whose path resolved: what it binds (`None` when the member it
/// names is not found there), and the MODULE / PACKAGE its file's IMPORTS
/// edge points at.
#[derive(Clone, Copy)]
struct UseHit {
    target: Option<NodeId>,
    imports: Option<NodeId>,
}

/// Every `use` of one Rust graph whose `from_module` names a node, as leaves
/// in statement order.
fn use_leaves<'a>(g: &RepoGraph, imports: &'a [ImportStmt]) -> Vec<UseLeaf<'a>> {
    let inner = inner_scopes(g);
    imports
        .iter()
        .filter_map(|stmt| {
            let q = stmt.from_module.as_str();
            let scope = g
                .symbols
                .module_by_qname
                .get(q)
                .copied()
                .or_else(|| inner.get(q).copied().flatten())?;
            let unnamed = |b: &&str| *b != "_";
            Some(match &stmt.target {
                ImportTarget::Symbol {
                    module,
                    name,
                    alias,
                    ..
                } => {
                    let path = split_path(module);
                    let glob = name == "*";
                    let first = path.as_ref().and_then(|p| p.first()).map(String::as_str);
                    UseLeaf {
                        scope,
                        tail_fallback: !glob && matches!(first, Some("crate" | "super")),
                        path,
                        member: (!glob).then_some(name.as_str()),
                        bound: (!glob)
                            .then(|| alias.as_deref().unwrap_or(name.as_str()))
                            .filter(unnamed),
                        glob,
                        alias: alias.is_some(),
                    }
                }
                ImportTarget::Module { path, alias } => UseLeaf {
                    scope,
                    tail_fallback: false,
                    path: split_path(path),
                    member: None,
                    bound: alias
                        .as_deref()
                        .or_else(|| path.rsplit("::").next())
                        .filter(unnamed),
                    glob: false,
                    alias: alias.is_some(),
                },
            })
        })
        .collect()
}

/// qname -> the inline PACKAGE, else the one fn / METHOD, of that qname: the
/// non-file scopes a `use` can sit in. A qname two fn-like nodes share maps
/// to `None` (never first-wins).
fn inner_scopes(g: &RepoGraph) -> HashMap<&str, Option<NodeId>> {
    let mut by_q: HashMap<&str, (Option<NodeId>, Vec<NodeId>)> = HashMap::new();
    for (id, kind) in &g.nav.kind_by_id {
        let is_pkg = *kind == node_kind::PACKAGE;
        if !is_pkg && *kind != node_kind::FUNCTION && *kind != node_kind::METHOD {
            continue;
        }
        let Some(q) = g.nav.qname_by_id.get(id) else {
            continue;
        };
        let slot = by_q.entry(q.as_str()).or_default();
        if is_pkg {
            slot.0 = Some(*id);
        } else {
            slot.1.push(*id);
        }
    }
    by_q.into_iter()
        .map(|(q, (pkg, fns))| {
            let fn_scope = match fns.as_slice() {
                [only] => Some(*only),
                _ => None,
            };
            (q, pkg.or(fn_scope))
        })
        .collect()
}

/// Per-build tallies of the `[rust-uses]` marker.
#[derive(Default)]
struct UseStats {
    scopes: usize,
    bindings: usize,
    hops: usize,
    alias: usize,
    fn_scoped: usize,
    glob: usize,
    unresolved: usize,
    imports_edges: usize,
}

/// Resolve one Rust graph's `use` trees (LA.1b), replacing
/// `resolve_imports_python` for Rust. Each leaf resolves as a Rust path from
/// the scope that holds it: the first segment through LA.1a's rules (an
/// earlier `use`, a child module, a type, a glob, a workspace crate), then
/// the member in that module: an item, else the module's own `use` binding
/// (a `pub use` re-export, followed), else a child module, else its globs.
///
/// Re-export chains are order-independent by construction: every leaf
/// resolves against the previous round's bindings, rounds repeat until the
/// tables stop changing, and a leaf needing more than [`MAX_USE_HOPS`] rounds
/// (a longer chain, or a cycle, which never settles on a target) stays
/// unresolved. A leaf never resolves through the name it binds.
///
/// Storage: a file MODULE's or inline PACKAGE's bindings go in
/// `module_import_bindings` (the generic resolver reads them, and the store
/// persists them); a fn's, and every glob, in the returned [`RustBindings`].
/// IMPORTS edges run from the leaf's file MODULE to the MODULE / PACKAGE the
/// path names (the member when it is a module, else its parent path), one per
/// (from, to), in statement order. A `crate::` / `super::` symbol in a file
/// whose crate is unknown (loose `.rs` files) that resolves to nothing keeps
/// HEAD's tail fallback (a repo-unique fn / module of that name); an external
/// crate's path binds nothing and draws no edge.
///
/// fired_on marker, once per graph with a `use`:
/// `[rust-uses] scopes=S bindings=B (reexport_hops=H alias=A fn_scoped=F
/// glob=G) unresolved=U imports_edges=E`. S = scopes holding a `use`, B =
/// distinct (scope, name) bound, H = bindings the final round's lookups went
/// through, A / F = bound leaves with an alias / in a fn body, G = globs
/// resolved to a module, U = leaves that bound nothing, E = IMPORTS edges.
pub(crate) fn resolve_imports_rust(
    g: &mut RepoGraph,
    imports: &[ImportStmt],
    idx: &RustIndex,
) -> RustBindings {
    let leaves = use_leaves(g, imports);
    if leaves.is_empty() {
        return RustBindings::default();
    }
    let mut tables = UseTables::default();
    let mut results: Vec<Option<UseHit>> = Vec::with_capacity(leaves.len());
    let mut hops: Vec<usize> = Vec::with_capacity(leaves.len());
    for _ in 0..=MAX_USE_HOPS {
        results.clear();
        hops.clear();
        for leaf in &leaves {
            let round = Round {
                tables: &tables,
                skip: leaf.bound.map(|b| (leaf.scope, b)),
                hops: Cell::new(0),
            };
            results.push(idx.resolve_leaf(g, &round, leaf));
            hops.push(round.hops.get());
        }
        let next = UseTables::collect(g, &leaves, &results);
        let settled = next == tables;
        tables = next;
        if settled {
            break;
        }
    }
    // HEAD's tail fallback, last resort, for the paths HEAD resolved, in a
    // file whose crate is unknown: no Cargo package covers it, or its package
    // has no crate root above it (loose `.rs` files, whose module tree is a
    // guess). In a known crate a `crate::` path that misses names something
    // the walk never saw (a gated dir, a macro-made item): a repo-wide guess
    // there would bind another crate's same-named fn.
    for (leaf, hit) in leaves.iter().zip(results.iter_mut()) {
        let bound = hit.is_some_and(|h| h.target.is_some());
        let (false, true, Some(name)) = (bound, leaf.tail_fallback, leaf.member) else {
            continue;
        };
        let known_crate = enclosing(&g.nav, leaf.scope, false)
            .and_then(|m| g.nav.qname_by_id.get(&m))
            .is_some_and(|q| {
                idx.covering(q).is_some() && matches!(idx.crate_root(g, q), Some(Root::Module(_)))
            });
        if known_crate {
            continue;
        }
        let Some(t) = unique_global_function(g, name).or_else(|| unique_global_module(g, name))
        else {
            continue;
        };
        *hit = Some(UseHit {
            target: Some(t),
            imports: Some(t),
        });
        if let Some(b) = leaf.bound {
            tables
                .bound
                .entry(leaf.scope)
                .or_default()
                .entry(b.to_string())
                .or_insert(t);
        }
    }

    let mut stats = UseStats::default();
    let mut scopes: HashSet<NodeId> = HashSet::new();
    for ((leaf, hit), h) in leaves.iter().zip(&results).zip(&hops) {
        scopes.insert(leaf.scope);
        if !hit.is_some_and(|h| h.target.is_some()) {
            stats.unresolved += 1;
            continue;
        }
        stats.hops += h;
        if leaf.glob {
            stats.glob += 1;
        } else if leaf.bound.is_some() {
            stats.alias += usize::from(leaf.alias);
            stats.fn_scoped += usize::from(is_fn_kind(&g.nav, leaf.scope));
        }
    }
    stats.scopes = scopes.len();

    let mut rb = RustBindings {
        fn_scoped: HashMap::new(),
        globs: tables.globs,
    };
    for (scope, names) in tables.bound {
        if names.is_empty() {
            continue;
        }
        stats.bindings += names.len();
        if is_fn_kind(&g.nav, scope) {
            rb.fn_scoped.insert(scope, names);
        } else {
            g.symbols
                .module_import_bindings
                .entry(scope)
                .or_default()
                .extend(names);
        }
    }
    let mut drawn: HashSet<(NodeId, NodeId)> = HashSet::new();
    for (leaf, hit) in leaves.iter().zip(&results) {
        let Some(UseHit {
            imports: Some(to), ..
        }) = hit
        else {
            continue;
        };
        let Some(from) = enclosing(&g.nav, leaf.scope, false) else {
            continue;
        };
        if from != *to && drawn.insert((from, *to)) {
            push_edge(g, from, *to, edge_category::IMPORTS);
        }
    }
    stats.imports_edges = drawn.len();
    eprintln!(
        "[rust-uses] scopes={} bindings={} (reexport_hops={} alias={} fn_scoped={} glob={}) \
         unresolved={} imports_edges={}",
        stats.scopes,
        stats.bindings,
        stats.hops,
        stats.alias,
        stats.fn_scoped,
        stats.glob,
        stats.unresolved,
        stats.imports_edges,
    );
    rb
}

impl RustIndex {
    /// One leaf against one round's bindings: its path to a module or type,
    /// then (a Symbol) the member it names there. `None` when the path itself
    /// does not resolve (an external crate); a path that resolves draws its
    /// IMPORTS edge even when the member it names is not found.
    fn resolve_leaf(
        &self,
        g: &RepoGraph,
        sc: &dyn UseScopes,
        leaf: &UseLeaf<'_>,
    ) -> Option<UseHit> {
        let path = leaf.path.as_ref()?;
        let scope = enclosing(&g.nav, leaf.scope, true)?;
        let file_module = enclosing(&g.nav, leaf.scope, false)?;
        let (start, _) = self.start(g, sc, leaf.scope, scope, file_module, &path[0])?;
        let mut unused = false;
        let pos = self.walk(g, sc, start, &path[1..], &mut unused)?;
        let module = match pos {
            Pos::Scope(id) => Some(id),
            _ => None,
        };
        if leaf.glob {
            return Some(UseHit {
                target: module,
                imports: module,
            });
        }
        let Some(name) = leaf.member else {
            let target = match pos {
                Pos::Scope(id) | Pos::Type(id) => Some(id),
                Pos::Virtual | Pos::TypeName(_) => None,
            };
            return Some(UseHit {
                target,
                imports: module,
            });
        };
        let target = self.use_member(g, sc, &pos, name);
        let imports = match target {
            Some(t) if g.nav.kind_by_id.get(&t).is_some_and(|k| is_scope_kind(*k)) => Some(t),
            _ => module,
        };
        Some(UseHit { target, imports })
    }

    /// The item `use <pos>::name` binds: an item the module defines, else its
    /// own `use` binding (a re-export), else a child module, else a name its
    /// globs bring in, else (at a crate root) the one top-level def of that
    /// name in the crate. A type's members (`use Kind::A`) bind nothing.
    fn use_member(
        &self,
        g: &RepoGraph,
        sc: &dyn UseScopes,
        pos: &Pos,
        name: &str,
    ) -> Option<NodeId> {
        let bindable = |id: &NodeId| g.nav.kind_by_id.get(id).is_some_and(|k| is_bindable(*k));
        match pos {
            Pos::Scope(id) => g
                .symbols
                .module_symbols
                .get(id)
                .and_then(|s| s.get(name))
                .copied()
                .filter(bindable)
                .or_else(|| {
                    let hit = sc.bound(g, *id, name).filter(bindable);
                    hit.inspect(|_| sc.note_hop())
                })
                .or_else(|| match self.child_pos(g, pos, name)? {
                    Pos::Scope(child) => Some(child),
                    _ => None,
                })
                .or_else(|| self.glob_member(g, sc, *id, name, is_bindable))
                .or_else(|| self.unique_in_crate(g, *id, name, false))
                .or_else(|| self.unique_in_crate(g, *id, name, true)),
            Pos::Virtual => match self.child_pos(g, pos, name)? {
                Pos::Scope(child) => Some(child),
                _ => None,
            },
            Pos::Type(_) | Pos::TypeName(_) => None,
        }
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

/// The field a receiver reads off `self` (LA.35b): `self.f` as written, or a
/// local the caller recorded as the alias `self.f` (`let c = &self.f`). A
/// chain, any other receiver, or a local of another / unknown type -> `None`.
fn self_field<'g>(g: &'g RepoGraph, site: &'g CallSite) -> Option<&'g str> {
    let CallQualifier::ComplexReceiver { receiver, .. } = &site.qualifier else {
        return None;
    };
    let field = match receiver.strip_prefix("self.") {
        Some(f) => f,
        None => g
            .nav
            .local_types
            .get(&site.from)?
            .get(receiver.as_str())?
            .strip_prefix("self.")?,
    };
    let ident = !field.is_empty() && field.chars().all(|c| c.is_alphanumeric() || c == '_');
    ident.then_some(field)
}

/// `scope`, then each enclosing MODULE / PACKAGE out to `file_module`
/// inclusive: the scopes a name written in `scope` is looked up in.
fn scope_chain(nav: &CodeNav, scope: NodeId, file_module: NodeId) -> Vec<NodeId> {
    let mut out = vec![scope];
    let mut cur = scope;
    while cur != file_module {
        let Some(up) = nav
            .parent_of
            .get(&cur)
            .and_then(|p| enclosing(nav, *p, true))
        else {
            break;
        };
        out.push(up);
        cur = up;
    }
    out
}

/// A module / inline mod as a scope, a type as a type; anything else `None`.
fn scope_or_type(g: &RepoGraph, id: NodeId) -> Option<Pos> {
    let kind = *g.nav.kind_by_id.get(&id)?;
    if is_scope_kind(kind) {
        Some(Pos::Scope(id))
    } else if is_type_kind(kind) {
        Some(Pos::Type(id))
    } else {
        None
    }
}

/// A fn or method: the scope of a `use` inside its body.
fn is_fn_kind(nav: &CodeNav, id: NodeId) -> bool {
    nav.kind_by_id
        .get(&id)
        .is_some_and(|k| *k == node_kind::FUNCTION || *k == node_kind::METHOD)
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

/// What a `use` binds a name to: an item, a module or an inline mod. Never a
/// METHOD (an `impl` elsewhere shares its file's symbol table) or an enum
/// variant (`use Kind::A` would turn a tuple-variant constructor into a CALLS
/// edge; LA.3 reads variants as USES).
fn is_bindable(k: NodeKindId) -> bool {
    k == node_kind::FUNCTION || k == node_kind::STATE_VAR || is_type_kind(k) || is_scope_kind(k)
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

    // ---- LA.35b: typed receivers through the hook -------------------------

    use crate::build::build_rust;
    use crate::test_support::repo;
    use repo_graph_code_domain::{FileParse, GRAPH_TYPE};
    use repo_graph_core::{Confidence, Node};

    /// One parsed file, built by hand in the Rust parser's shapes.
    struct File {
        module: NodeId,
        nav: CodeNav,
        nodes: Vec<Node>,
        imports: Vec<ImportStmt>,
        calls: Vec<CallSite>,
    }

    impl File {
        fn new(module_q: &str) -> Self {
            let mut f = File {
                module: NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, module_q),
                nav: CodeNav::default(),
                nodes: vec![],
                imports: vec![],
                calls: vec![],
            };
            f.add(node_kind::MODULE, module_q, None);
            f
        }

        fn add(&mut self, kind: NodeKindId, qname: &str, parent: Option<NodeId>) -> NodeId {
            let id = NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname);
            self.nav.record(id, last_seg(qname), qname, kind, parent);
            self.nodes.push(Node {
                id,
                repo: repo(),
                confidence: Confidence::Strong,
                cells: vec![],
            });
            id
        }

        /// `use <module>::<name>;` written in this file.
        fn use_item(&mut self, module: &str, name: &str) {
            let from_module = self.nav.qname_by_id[&self.module].clone();
            self.imports.push(ImportStmt {
                from_module,
                target: ImportTarget::Symbol {
                    module: module.to_string(),
                    name: name.to_string(),
                    alias: None,
                    level: 0,
                },
            });
        }

        /// `<receiver>.<name>()` written in `from`.
        fn call(&mut self, from: NodeId, receiver: &str, name: &str) {
            self.calls.push(CallSite {
                from,
                qualifier: CallQualifier::ComplexReceiver {
                    receiver: receiver.to_string(),
                    name: name.to_string(),
                },
            });
        }

        fn parse(self) -> FileParse {
            FileParse {
                nodes: self.nodes,
                edges: vec![],
                imports: self.imports,
                calls: self.calls,
                refs: vec![],
                nav: self.nav,
                properties: HashSet::new(),
            }
        }
    }

    fn krate(name: &str, dir: &str) -> RustCrate {
        let lib = if dir.is_empty() {
            "src::lib".to_string()
        } else {
            format!("{dir}::src::lib")
        };
        RustCrate {
            name: name.to_string(),
            dir: dir.to_string(),
            lib_root: Some(lib),
            other_roots: vec![],
        }
    }

    /// Every CALLS edge out of `from`, as target qnames, sorted.
    fn calls_from(g: &RepoGraph, from: NodeId) -> Vec<String> {
        let mut out: Vec<String> = g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::CALLS && e.from == from)
            .filter_map(|e| g.nav.qname_by_id.get(&e.to).cloned())
            .collect();
        out.sort();
        out
    }

    /// `struct Cache` in `<p>::repo`, its `lookup` in an `impl Cache` in
    /// `<p>::cache_impl` (parented to that MODULE, as the parser does).
    fn cache_crate(p: &str) -> Vec<FileParse> {
        let mut repo_rs = File::new(&format!("{p}::repo"));
        let m = repo_rs.module;
        repo_rs.add(node_kind::STRUCT, &format!("{p}::repo::Cache"), Some(m));
        let mut impl_rs = File::new(&format!("{p}::cache_impl"));
        let m = impl_rs.module;
        impl_rs.use_item("crate::repo", "Cache");
        impl_rs.add(
            node_kind::METHOD,
            &format!("{p}::cache_impl::Cache::lookup"),
            Some(m),
        );
        vec![repo_rs.parse(), impl_rs.parse()]
    }

    #[test]
    fn typed_receiver_binds_impl_in_another_file() {
        let mut lib = File::new("src::lib");
        let m = lib.module;
        lib.use_item("crate::repo", "Cache");
        let svc = lib.add(node_kind::STRUCT, "src::lib::Service", Some(m));
        lib.nav.record_field_type(svc, "cache", "Cache");
        let cached = lib.add(node_kind::METHOD, "src::lib::Service::cached", Some(svc));
        lib.call(cached, "self.cache", "lookup");
        let free = lib.add(node_kind::FUNCTION, "src::lib::free", Some(m));
        lib.nav.record_local_type(free, "c", "Cache");
        lib.call(free, "c", "lookup");
        let mut parses = cache_crate("src");
        parses.push(lib.parse());
        let g = build_rust(repo(), parses, &[krate("recv", "")]).expect("builds");
        let lookup = vec!["src::cache_impl::Cache::lookup".to_string()];
        assert_eq!(calls_from(&g, cached), lookup, "self.cache: Cache field");
        assert_eq!(calls_from(&g, free), lookup, "c: Cache parameter");
    }

    #[test]
    fn self_field_in_a_cross_file_impl_uses_the_qname_owner() {
        // `struct Service { repo: Repo }` in lib.rs, which imports the
        // `Repo` of repo.rs; `impl Service` in service_impl.rs, which imports
        // another `Repo`. `self.repo` has the type the STRUCT's file names.
        let mut repo_rs = File::new("src::repo");
        let m = repo_rs.module;
        let repo_ty = repo_rs.add(node_kind::STRUCT, "src::repo::Repo", Some(m));
        repo_rs.add(node_kind::METHOD, "src::repo::Repo::find", Some(repo_ty));
        let mut other_rs = File::new("src::other");
        let m = other_rs.module;
        let other_ty = other_rs.add(node_kind::STRUCT, "src::other::Repo", Some(m));
        other_rs.add(node_kind::METHOD, "src::other::Repo::find", Some(other_ty));
        let mut lib = File::new("src::lib");
        let m = lib.module;
        lib.use_item("crate::repo", "Repo");
        let svc = lib.add(node_kind::STRUCT, "src::lib::Service", Some(m));
        lib.nav.record_field_type(svc, "repo", "Repo");
        let mut imp = File::new("src::service_impl");
        let m = imp.module;
        imp.use_item("crate::other", "Repo");
        let get = imp.add(
            node_kind::METHOD,
            "src::service_impl::Service::get",
            Some(m),
        );
        imp.call(get, "self.repo", "find");
        let aliased = imp.add(
            node_kind::METHOD,
            "src::service_impl::Service::aliased",
            Some(m),
        );
        imp.nav.record_local_type(aliased, "r", "self.repo");
        imp.call(aliased, "r", "find");
        let parses = vec![repo_rs.parse(), other_rs.parse(), lib.parse(), imp.parse()];
        let g = build_rust(repo(), parses, &[krate("recv", "")]).expect("builds");
        let find = vec!["src::repo::Repo::find".to_string()];
        assert_eq!(calls_from(&g, get), find, "self.repo in a cross-file impl");
        assert_eq!(calls_from(&g, aliased), find, "let r = &self.repo");
    }

    #[test]
    fn enum_typed_receiver_binds_enum_method() {
        // Two crates each define `enum Mode` with `label` on it; crate `a`
        // names its own through a glob, which the generic pass cannot read,
        // and `describe` sits in an `impl Mode` in another file.
        let mut parses = Vec::new();
        for p in ["a", "b"] {
            let mut mode = File::new(&format!("{p}::src::mode"));
            let m = mode.module;
            let e = mode.add(node_kind::ENUM, &format!("{p}::src::mode::Mode"), Some(m));
            mode.add(
                node_kind::ATTRIBUTE,
                &format!("{p}::src::mode::Mode::On"),
                Some(e),
            );
            mode.add(
                node_kind::METHOD,
                &format!("{p}::src::mode::Mode::label"),
                Some(e),
            );
            let mut imp = File::new(&format!("{p}::src::mode_impl"));
            let m = imp.module;
            imp.use_item("crate::mode", "Mode");
            imp.add(
                node_kind::METHOD,
                &format!("{p}::src::mode_impl::Mode::describe"),
                Some(m),
            );
            parses.extend([mode.parse(), imp.parse()]);
        }
        let mut lib = File::new("a::src::lib");
        let m = lib.module;
        lib.use_item("crate::mode", "*");
        let run = lib.add(node_kind::FUNCTION, "a::src::lib::run", Some(m));
        lib.nav.record_local_type(run, "m", "Mode");
        lib.call(run, "m", "label");
        lib.call(run, "m", "describe");
        parses.push(lib.parse());
        let crates = [krate("a", "a"), krate("b", "b")];
        let g = build_rust(repo(), parses, &crates).expect("builds");
        assert_eq!(
            calls_from(&g, run),
            vec![
                "a::src::mode::Mode::label",
                "a::src::mode_impl::Mode::describe"
            ],
        );
    }

    #[test]
    fn shadowed_local_is_not_resolved_by_the_hook() {
        // `let repo = index();` (type unknown) shadows `self.repo`, in an
        // `impl` in another file and in one beside its type.
        let mut repo_rs = File::new("src::repo");
        let m = repo_rs.module;
        let repo_ty = repo_rs.add(node_kind::STRUCT, "src::repo::Repo", Some(m));
        repo_rs.add(node_kind::METHOD, "src::repo::Repo::find", Some(repo_ty));
        let mut lib = File::new("src::lib");
        let m = lib.module;
        lib.use_item("crate::repo", "Repo");
        let svc = lib.add(node_kind::STRUCT, "src::lib::Service", Some(m));
        lib.nav.record_field_type(svc, "repo", "Repo");
        let near = lib.add(node_kind::METHOD, "src::lib::Service::near", Some(svc));
        lib.nav.record_local_type(near, "repo", "");
        lib.call(near, "repo", "find");
        let mut imp = File::new("src::service_impl");
        let m = imp.module;
        let far = imp.add(
            node_kind::METHOD,
            "src::service_impl::Service::far",
            Some(m),
        );
        imp.nav.record_local_type(far, "repo", "");
        imp.call(far, "repo", "find");
        let parses = vec![repo_rs.parse(), lib.parse(), imp.parse()];
        let g = build_rust(repo(), parses, &[krate("recv", "")]).expect("builds");
        assert!(calls_from(&g, near).is_empty());
        assert!(calls_from(&g, far).is_empty());
    }

    #[test]
    fn ambiguous_impl_elsewhere_is_unresolved() {
        // Crates `a` and `b` each `impl Cache { fn lookup }` in another file;
        // `c` names no `Cache`, so its `cache.lookup()` binds neither. Inside
        // `d`, two `Cache` types each with an other-file `lookup` are a tie.
        let mut parses = cache_crate("a::src");
        parses.extend(cache_crate("b::src"));
        let mut lib = File::new("c::src::lib");
        let m = lib.module;
        let run = lib.add(node_kind::FUNCTION, "c::src::lib::run", Some(m));
        lib.nav.record_local_type(run, "cache", "Cache");
        lib.call(run, "cache", "lookup");
        parses.push(lib.parse());
        for sub in ["x", "y"] {
            parses.extend(cache_crate(&format!("d::src::{sub}")));
        }
        let mut dlib = File::new("d::src::lib");
        let m = dlib.module;
        let drun = dlib.add(node_kind::FUNCTION, "d::src::lib::run", Some(m));
        dlib.nav.record_local_type(drun, "cache", "Cache");
        dlib.call(drun, "cache", "lookup");
        parses.push(dlib.parse());
        let crates = [
            krate("a", "a"),
            krate("b", "b"),
            krate("c", "c"),
            krate("d", "d"),
        ];
        let g = build_rust(repo(), parses, &crates).expect("builds");
        assert!(calls_from(&g, run).is_empty(), "{:?}", calls_from(&g, run));
        assert!(
            calls_from(&g, drun).is_empty(),
            "{:?}",
            calls_from(&g, drun)
        );
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
