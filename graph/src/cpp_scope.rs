//! C++ call scope (CB.25): a bare call inside a member binds the class's own
//! member first (implicit `this`), `Type::m()` / `ns::f()` bind through C++
//! name lookup, typed receivers resolve through `using` directives, and a call
//! through a header prototype binds the one external definition. Crate-private:
//! [`crate::build::build_c_cpp`] runs [`implicit_this`] before `resolve_calls`
//! and consults [`CppScope::resolve`] from its extra-hook after LB.10a/c's
//! `CppCallScope`, so every rule but implicit `this` runs only when the
//! generic lookups, the defining file and the direct includes found nothing.
//!
//! What a call sees is C++'s textual inclusion: the caller's file and every
//! file its `#include`s reach, transitively ([`CppScope::visible`]: the
//! IMPORTS edges the include resolver bound, at most [`INCLUDE_DEPTH`] hops).
//! A bound out-of-line member's file is its DEFINING file (`cart.cpp`), not
//! its class's header. A name is looked up from the caller's C++ scope
//! outward (`shop::Cart` -> `shop` -> global), then through the `using`
//! declarations and directives (CB.19's `NavFact`s) of the visible files that
//! apply at that scope. Everything is name-based, as `class_methods` is: an
//! overload set is one METHOD.
//!
//! A prototype (`int f();` in a header) names the definition of `f` with
//! external linkage in a source file (`.c` / `.cc` / `.cpp` / `.cxx`); a
//! `static` or anonymous-namespace one never answers. When several programs
//! of one repo each define `f`, the one in the directory of a visible header
//! declaring it binds (`utils.h` beside `utils.cpp`), as LB.10c breaks a type
//! tie by directory; any other tie is refused and counted `ambiguous`.
//!
//! The parser records a receiver's type as its last `::` segment
//! (`legacy::Cart c;` -> `Cart`, CB.19), so a typed receiver is re-qualified
//! by the lookup above: under `using namespace shop;` that reads `shop::Cart`
//! even where the source spelled `legacy::Cart`.
//!
//! fired_on marker, once per C/C++ graph:
//! `[cpp-scope] implicit_this=<a> qualified=<b> receiver_using=<c> prototype=<d> using=<e> ambiguous=<f>`
//! — grep `^\[cpp-scope\]`.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{CallQualifier, CallSite, CodeNav, NavFact, edge_category, node_kind};
use glia_core::NodeId;

use crate::build::{cpp_join, cpp_name, cpp_ns_prefixes, is_cpp_type};
use crate::calls::{enclosing_class_or_struct, enclosing_module, graph_evidence, receiver_type};
use crate::types::RepoGraph;

/// The EVIDENCE emitter of every edge the hook binds.
const EMITTER: &str = "graph:cpp_scope";
/// How many `#include` hops [`CppScope::visible`] follows.
const INCLUDE_DEPTH: usize = 8;
/// How many base-class levels a member lookup climbs.
const BASE_DEPTH: usize = 4;
/// Extensions of a translation unit: a function one defines with external
/// linkage is the definition a prototype names.
const SOURCE_EXTS: [&str; 4] = ["c", "cc", "cpp", "cxx"];

/// Evidence rules, one per resolution shape (the marker's fields).
const IMPLICIT_THIS: &str = "implicit_this";
const QUALIFIED: &str = "qualified";
const RECEIVER_USING: &str = "receiver_using";
const PROTOTYPE: &str = "prototype";
const USING: &str = "using";

/// A namespace-scope FUNCTION: its file MODULE, its C++ namespace and
/// whether it is a definition a prototype elsewhere can name (a source file's
/// function without internal linkage).
struct FnDef {
    id: NodeId,
    module: NodeId,
    /// Its MODULE's directory ([`module_dir`]).
    dir: String,
    ns: String,
    external: bool,
}

/// A prototype (`DeclaresFn`) of one file: its C++ namespace, its MODULE and
/// that MODULE's directory.
struct Proto {
    ns: String,
    module: NodeId,
    dir: String,
}

/// One `using` of a file: a declaration (`using shop::Cart;`, `name` set) or
/// a directive (`using namespace shop;`), in effect inside `within`.
struct Using {
    module: NodeId,
    within: String,
    ns: String,
    name: Option<String>,
}

/// What one lookup found: a single node, several (refused, counted
/// ambiguous), or nothing.
#[derive(Debug, PartialEq, Eq)]
enum Found {
    One(NodeId),
    Many,
    Nothing,
}

impl Found {
    /// The distinct ids of `ids`, deduped by id so the answer never depends
    /// on the order they were collected in.
    fn of(mut ids: Vec<NodeId>) -> Self {
        ids.sort_unstable_by_key(|id| id.0);
        ids.dedup();
        match ids[..] {
            [one] => Found::One(one),
            [] => Found::Nothing,
            _ => Found::Many,
        }
    }

    /// Several lookups joined: any refused lookup refuses the whole, else
    /// the distinct hits decide.
    fn any(all: impl IntoIterator<Item = Found>) -> Self {
        let mut ones = Vec::new();
        for f in all {
            match f {
                Found::One(id) => ones.push(id),
                Found::Many => return Found::Many,
                Found::Nothing => {}
            }
        }
        Found::of(ones)
    }
}

/// A C/C++ graph's lookup tables for [`CppScope::resolve`] and
/// [`implicit_this`], and the counts of what they bound. Built once per
/// graph after the include resolver ran; BTree-ordered where a scan could
/// decide an answer, HashMaps for lookup only.
pub(crate) struct CppScope {
    /// Joined out-of-line member -> its defining file MODULE (LB.10c).
    member_file: HashMap<NodeId, NodeId>,
    /// Including MODULE -> the MODULEs its IMPORTS edges reach, edge order.
    includes: HashMap<NodeId, Vec<NodeId>>,
    /// MODULE -> its visible set, filled on first use.
    closures: RefCell<HashMap<NodeId, Rc<HashSet<NodeId>>>>,
    /// C++ name -> every CLASS / STRUCT of that name with its MODULE,
    /// `g.nodes` order.
    types: BTreeMap<String, Vec<(NodeId, NodeId)>>,
    /// C++ names of every namespace a PACKAGE, a prototype or a definition
    /// names, with their outer namespaces.
    namespaces: BTreeSet<String>,
    /// Function name -> the namespace-scope FUNCTIONs of that name.
    fns: BTreeMap<String, Vec<FnDef>>,
    /// Function name -> each prototype of it.
    prototypes: BTreeMap<String, Vec<Proto>>,
    /// Every file's `using`s, `g.nodes` order then fact order.
    usings: Vec<Using>,
    /// CLASS / STRUCT -> its bases, from INHERITS_FROM edges already in the
    /// graph when the pass runs, edge order.
    bases: HashMap<NodeId, Vec<NodeId>>,
    qualified: Cell<usize>,
    receiver_using: Cell<usize>,
    prototype: Cell<usize>,
    using: Cell<usize>,
    ambiguous: Cell<usize>,
}

impl CppScope {
    /// Index `g` after `resolve_imports_ts` (its IMPORTS edges are the
    /// include graph). `defining` is [`crate::build`]'s LB.10c map, joined
    /// member -> its defining file's lexical scope.
    pub(crate) fn new(g: &RepoGraph, defining: &HashMap<NodeId, NodeId>) -> Self {
        let nav = &g.nav;
        let member_file = defining
            .iter()
            .filter_map(|(member, scope)| Some((*member, enclosing_module(nav, *scope)?)))
            .collect();
        let mut includes: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut bases: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for e in &g.edges {
            let slot = if e.category == edge_category::IMPORTS {
                includes.entry(e.from).or_default()
            } else if e.category == edge_category::INHERITS_FROM
                && is_cpp_type(nav, e.from)
                && is_cpp_type(nav, e.to)
            {
                bases.entry(e.from).or_default()
            } else {
                continue;
            };
            if !slot.contains(&e.to) {
                slot.push(e.to);
            }
        }

        // The file facts CB.19 records on each MODULE.
        let mut namespaces: BTreeSet<String> = BTreeSet::new();
        let mut prototypes: BTreeMap<String, Vec<Proto>> = BTreeMap::new();
        let mut internal: HashMap<NodeId, HashSet<&str>> = HashMap::new();
        let mut usings: Vec<Using> = Vec::new();
        for n in &g.nodes {
            if nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                continue;
            }
            for fact in nav.nav_facts.get(&n.id).into_iter().flatten() {
                match fact {
                    NavFact::DeclaresFn { ns, name } => {
                        add_namespace(&mut namespaces, ns);
                        prototypes.entry(name.clone()).or_default().push(Proto {
                            ns: ns.clone(),
                            module: n.id,
                            dir: module_dir(nav, n.id),
                        });
                    }
                    NavFact::InternalLinkage { name } => {
                        internal.entry(n.id).or_default().insert(name.as_str());
                    }
                    NavFact::UsingNamespace { within, ns } => usings.push(Using {
                        module: n.id,
                        within: within.clone(),
                        ns: ns.clone(),
                        name: None,
                    }),
                    NavFact::UsingName { within, ns, name } => usings.push(Using {
                        module: n.id,
                        within: within.clone(),
                        ns: ns.clone(),
                        name: Some(name.clone()),
                    }),
                    _ => {}
                }
            }
        }

        let mut types: BTreeMap<String, Vec<(NodeId, NodeId)>> = BTreeMap::new();
        let mut fns: BTreeMap<String, Vec<FnDef>> = BTreeMap::new();
        for n in &g.nodes {
            let Some(&kind) = nav.kind_by_id.get(&n.id) else {
                continue;
            };
            if kind == node_kind::PACKAGE {
                add_namespace(&mut namespaces, &cpp_name(nav, n.id));
                continue;
            }
            let Some(module) = enclosing_module(nav, n.id) else {
                continue;
            };
            if is_cpp_type(nav, n.id) {
                types
                    .entry(cpp_name(nav, n.id))
                    .or_default()
                    .push((n.id, module));
                continue;
            }
            if kind != node_kind::FUNCTION || !at_namespace_scope(nav, n.id) {
                continue;
            }
            // An out-of-line definition LB.10c could not place keeps its
            // qualifier in its name (`Outer::Inner::m`): no free function.
            let Some(name) = nav.name_by_id.get(&n.id).filter(|s| !s.contains("::")) else {
                continue;
            };
            let ns = def_scope(nav, n.id);
            add_namespace(&mut namespaces, &ns);
            let external = is_source(nav, module)
                && !internal
                    .get(&module)
                    .is_some_and(|names| names.contains(name.as_str()));
            fns.entry(name.clone()).or_default().push(FnDef {
                id: n.id,
                module,
                dir: module_dir(nav, module),
                ns,
                external,
            });
        }

        CppScope {
            member_file,
            includes,
            closures: RefCell::new(HashMap::new()),
            types,
            namespaces,
            fns,
            prototypes,
            usings,
            bases,
            qualified: Cell::new(0),
            receiver_using: Cell::new(0),
            prototype: Cell::new(0),
            using: Cell::new(0),
            ambiguous: Cell::new(0),
        }
    }

    /// `resolve_calls`' extra-hook, after every generic lookup and LB.10a/c's
    /// `CppCallScope` missed:
    /// - `SelfMethod(m)` (`this->m()`, or a bare call [`implicit_this`]
    ///   rewrote): `m` of a base of the caller's class, rule `implicit_this`;
    /// - `Attribute` / `ComplexReceiver` with a typed receiver (CB.19's
    ///   locals and fields, [`receiver_type`]): the type through C++ lookup
    ///   and `using`, then its method, rule `receiver_using`;
    /// - `Attribute { base: T, name }`, `T` no variable of the caller: a type
    ///   (`Cart::count()`) -> its method, else a namespace (`util::clamp()`)
    ///   -> its function (a visible definition, else the one external
    ///   definition of a visible prototype), rule `qualified`;
    /// - `Bare(f)`: a visible prototype of `f` in one of the caller's
    ///   namespaces -> the one external definition, rule `prototype`; else a
    ///   `using` in effect -> that namespace's `f`, rule `using`.
    ///
    /// Several candidates are refused, never a first match.
    pub(crate) fn resolve(&self, g: &RepoGraph, site: &CallSite) -> Option<(NodeId, Evidence)> {
        match &site.qualifier {
            CallQualifier::SelfMethod(name) => {
                let owner = enclosing_class_or_struct(&g.nav, site.from)?;
                self.settle(self.base_member(g, owner, name), IMPLICIT_THIS)
            }
            CallQualifier::Bare(name) => self.bare(g, site.from, name),
            CallQualifier::Attribute { name, .. } | CallQualifier::ComplexReceiver { name, .. } => {
                self.member_call(g, site, name)
            }
            CallQualifier::SuperMethod(_) => None,
        }
    }

    /// The `[cpp-scope]` fired_on line, `implicit_this` being
    /// [`implicit_this`]'s count.
    pub(crate) fn marker(&self, implicit_this: usize) -> String {
        format!(
            "[cpp-scope] implicit_this={implicit_this} qualified={} receiver_using={} \
             prototype={} using={} ambiguous={}",
            self.qualified.get(),
            self.receiver_using.get(),
            self.prototype.get(),
            self.using.get(),
            self.ambiguous.get()
        )
    }

    /// A lookup's answer as the hook's: one node binds under `rule` (and is
    /// counted), several are counted ambiguous.
    fn settle(&self, found: Found, rule: &'static str) -> Option<(NodeId, Evidence)> {
        match found {
            Found::One(to) => {
                let count = match rule {
                    QUALIFIED => Some(&self.qualified),
                    RECEIVER_USING => Some(&self.receiver_using),
                    PROTOTYPE => Some(&self.prototype),
                    USING => Some(&self.using),
                    // A base member: the pre-pass counted the rewrite.
                    _ => None,
                };
                if let Some(c) = count {
                    c.set(c.get() + 1);
                }
                Some((to, graph_evidence(EMITTER, rule)))
            }
            Found::Many => {
                self.ambiguous.set(self.ambiguous.get() + 1);
                None
            }
            Found::Nothing => None,
        }
    }

    /// The file a call from `from` sees: a joined out-of-line member's
    /// defining file, else its own.
    fn caller_file(&self, g: &RepoGraph, from: NodeId) -> Option<NodeId> {
        self.member_file
            .get(&from)
            .copied()
            .or_else(|| enclosing_module(&g.nav, from))
    }

    /// `file` and every MODULE its includes reach, breadth first in edge
    /// order, at most [`INCLUDE_DEPTH`] hops; an include cycle ends at the
    /// visited set. Computed once per file.
    fn visible(&self, file: NodeId) -> Rc<HashSet<NodeId>> {
        if let Some(seen) = self.closures.borrow().get(&file) {
            return Rc::clone(seen);
        }
        let mut seen: HashSet<NodeId> = HashSet::from([file]);
        let mut frontier = vec![file];
        for _ in 0..INCLUDE_DEPTH {
            let mut next = Vec::new();
            for m in &frontier {
                for inc in self.includes.get(m).into_iter().flatten() {
                    if seen.insert(*inc) {
                        next.push(*inc);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        let seen = Rc::new(seen);
        self.closures.borrow_mut().insert(file, Rc::clone(&seen));
        seen
    }

    /// `name` of the nearest base of `class` declaring it, level by level
    /// (at most [`BASE_DEPTH`]); two bases of one level that both declare it
    /// are ambiguous.
    fn base_member(&self, g: &RepoGraph, class: NodeId, name: &str) -> Found {
        let mut seen: HashSet<NodeId> = HashSet::from([class]);
        let mut level: Vec<NodeId> = self.bases.get(&class).cloned().unwrap_or_default();
        for _ in 0..BASE_DEPTH {
            level.retain(|b| seen.insert(*b));
            if level.is_empty() {
                break;
            }
            let hits: Vec<NodeId> = level
                .iter()
                .filter_map(|b| g.symbols.class_methods.get(b)?.get(name).copied())
                .collect();
            if !hits.is_empty() {
                return Found::of(hits);
            }
            level = level
                .iter()
                .flat_map(|b| self.bases.get(b).into_iter().flatten().copied())
                .collect();
        }
        Found::Nothing
    }

    /// A `Bare` call the generic pass missed (C6, then C7).
    fn bare(&self, g: &RepoGraph, from: NodeId, name: &str) -> Option<(NodeId, Evidence)> {
        // `printf`, `memcpy`: no prototype in the repo and no `using` to
        // follow, so no include closure is worth computing.
        if self.usings.is_empty() && !self.prototypes.contains_key(name) {
            return None;
        }
        let vis = self.visible(self.caller_file(g, from)?);
        let scope = def_scope(&g.nav, from);
        // C6: the innermost of the caller's namespaces with a visible
        // prototype of `name` decides, as C++ lookup stops at the first
        // scope declaring it.
        if let Some(ns) = cpp_ns_prefixes(&scope)
            .into_iter()
            .find(|ns| self.declared(&vis, ns, name))
        {
            return self.settle(self.external(&vis, ns, name), PROTOTYPE);
        }
        // C7: a using-declaration of `name` before the using-directives.
        let usings = self.usings_at(&vis, &scope);
        let decls: Vec<Found> = usings
            .iter()
            .filter(|u| u.name.as_deref() == Some(name))
            .map(|u| self.ns_function(&vis, &self.using_ns(u), name))
            .collect();
        let found = match Found::any(decls) {
            Found::Nothing => Found::any(
                usings
                    .iter()
                    .filter(|u| u.name.is_none())
                    .map(|u| self.ns_function(&vis, &self.using_ns(u), name)),
            ),
            found => found,
        };
        self.settle(found, USING)
    }

    /// An `Attribute` / `ComplexReceiver` call the generic pass missed (C5,
    /// C7).
    fn member_call(
        &self,
        g: &RepoGraph,
        site: &CallSite,
        name: &str,
    ) -> Option<(NodeId, Evidence)> {
        if name.is_empty() {
            return None;
        }
        let vis = self.visible(self.caller_file(g, site.from)?);
        let scope = def_scope(&g.nav, site.from);
        // An object receiver of a known type: never read as a type name.
        if let Some(ty) = receiver_type(g, site) {
            let found = self.method_of(g, self.lookup_type(&vis, &scope, ty), name);
            return self.settle(found, RECEIVER_USING);
        }
        let CallQualifier::Attribute { base, .. } = &site.qualifier else {
            return None;
        };
        if names_a_variable(g, site.from, base) {
            return None;
        }
        // `::f()`: the global namespace.
        if base.is_empty() {
            return self.settle(self.ns_function(&vis, "", name), QUALIFIED);
        }
        match self.lookup_type(&vis, &scope, base) {
            Found::Nothing => {}
            found => return self.settle(self.method_of(g, found, name), QUALIFIED),
        }
        let ns = self.resolve_ns(&scope, base)?;
        self.settle(self.ns_function(&vis, &ns, name), QUALIFIED)
    }

    /// Method `name` of a type a lookup found.
    fn method_of(&self, g: &RepoGraph, found: Found, name: &str) -> Found {
        match found {
            Found::One(ty) => g
                .symbols
                .class_methods
                .get(&ty)
                .and_then(|m| m.get(name))
                .map_or(Found::Nothing, |m| Found::One(*m)),
            other => other,
        }
    }

    /// C++ lookup of the type `ty` (`Cart`, `a::Cart`, `::a::Cart`) from
    /// `scope`: a visible type named `<prefix>::ty` for each of the scope's
    /// prefixes, innermost first, the first that has one deciding; then the
    /// `using` declarations naming `ty`'s first segment, then the `using`
    /// directives in effect. A type no visible file declares is not found.
    fn lookup_type(&self, vis: &HashSet<NodeId>, scope: &str, ty: &str) -> Found {
        if ty.is_empty() {
            return Found::Nothing;
        }
        if let Some(abs) = ty.strip_prefix("::") {
            // An absolute name: the global namespace alone, no `using`.
            return self.visible_type(vis, abs).unwrap_or(Found::Nothing);
        }
        for p in cpp_ns_prefixes(scope) {
            if let Some(found) = self.visible_type(vis, &cpp_join(p, ty)) {
                return found;
            }
        }
        let head = ty.split("::").next().unwrap_or(ty);
        let usings = self.usings_at(vis, scope);
        let through = |decl: bool| {
            Found::any(
                usings
                    .iter()
                    .filter(|u| match &u.name {
                        Some(n) => decl && n == head,
                        None => !decl,
                    })
                    .filter_map(|u| self.visible_type(vis, &cpp_join(&self.using_ns(u), ty))),
            )
        };
        match through(true) {
            Found::Nothing => through(false),
            found => found,
        }
    }

    /// The visible types of C++ name `full`, `None` when no visible file
    /// declares one.
    fn visible_type(&self, vis: &HashSet<NodeId>, full: &str) -> Option<Found> {
        let here: Vec<NodeId> = self
            .types
            .get(full)?
            .iter()
            .filter(|(_, module)| vis.contains(module))
            .map(|(ty, _)| *ty)
            .collect();
        (!here.is_empty()).then(|| Found::of(here))
    }

    /// The namespace `n` (`util`, `a::b`, `::a`) names from `scope`: the
    /// first `<prefix>::n` a declaration knows, innermost first.
    fn resolve_ns(&self, scope: &str, n: &str) -> Option<String> {
        if let Some(abs) = n.strip_prefix("::") {
            return self.namespaces.contains(abs).then(|| abs.to_string());
        }
        cpp_ns_prefixes(scope)
            .into_iter()
            .map(|p| cpp_join(p, n))
            .find(|full| self.namespaces.contains(full))
    }

    /// The namespace a `using` names, resolved from where it is written
    /// (`using namespace detail;` inside `namespace shop` is `shop::detail`
    /// when that exists).
    fn using_ns(&self, u: &Using) -> String {
        self.resolve_ns(&u.within, &u.ns)
            .unwrap_or_else(|| u.ns.clone())
    }

    /// The `using`s of the visible files in effect at `scope`: written at
    /// file scope, or inside `scope` or one of its enclosing namespaces.
    fn usings_at(&self, vis: &HashSet<NodeId>, scope: &str) -> Vec<&Using> {
        self.usings
            .iter()
            .filter(|u| vis.contains(&u.module))
            .filter(|u| {
                u.within.is_empty()
                    || scope == u.within
                    || scope
                        .strip_prefix(u.within.as_str())
                        .is_some_and(|rest| rest.starts_with("::"))
            })
            .collect()
    }

    /// Function `name` of namespace `ns`: its visible definitions, else,
    /// when a visible prototype declares it there, its one external
    /// definition.
    fn ns_function(&self, vis: &HashSet<NodeId>, ns: &str, name: &str) -> Found {
        let here: Vec<NodeId> = self
            .fns
            .get(name)
            .into_iter()
            .flatten()
            .filter(|d| d.ns == ns && vis.contains(&d.module))
            .map(|d| d.id)
            .collect();
        if !here.is_empty() {
            return Found::of(here);
        }
        if self.declared(vis, ns, name) {
            return self.external(vis, ns, name);
        }
        Found::Nothing
    }

    /// True when a visible file declares a prototype of `ns::name`.
    fn declared(&self, vis: &HashSet<NodeId>, ns: &str, name: &str) -> bool {
        self.prototypes
            .get(name)
            .is_some_and(|ps| ps.iter().any(|p| p.ns == ns && vis.contains(&p.module)))
    }

    /// The definitions of `ns::name` with external linkage in a source file
    /// (a `static` / anonymous-namespace one never answers): one binds.
    /// Several (one per program of a repo, or per platform) bind only the one
    /// in the directory of a visible header declaring it (`utils.h` beside
    /// `utils.cpp`), as LB.10c's type join breaks a tie by directory; two
    /// there, or none, are ambiguous.
    fn external(&self, vis: &HashSet<NodeId>, ns: &str, name: &str) -> Found {
        let defs: Vec<&FnDef> = self
            .fns
            .get(name)
            .into_iter()
            .flatten()
            .filter(|d| d.ns == ns && d.external)
            .collect();
        let all = Found::of(defs.iter().map(|d| d.id).collect());
        if all != Found::Many {
            return all;
        }
        let dirs: BTreeSet<&str> = self
            .prototypes
            .get(name)
            .into_iter()
            .flatten()
            .filter(|p| p.ns == ns && vis.contains(&p.module))
            .map(|p| p.dir.as_str())
            .collect();
        match Found::of(
            defs.iter()
                .filter(|d| dirs.contains(d.dir.as_str()))
                .map(|d| d.id)
                .collect(),
        ) {
            Found::Nothing => Found::Many,
            beside => beside,
        }
    }
}

/// C5 (member-first, as C++ class-scope lookup): every `Bare(name)` call
/// from a METHOD whose enclosing CLASS / STRUCT declares a member `name`
/// (its own `class_methods`, or a base's through [`CppScope`]'s INHERITS_FROM
/// index) becomes `SelfMethod(name)`, in place, so the member wins over a
/// same-named free function the generic pass would bind through the file,
/// its namespace or a direct include. `resolve_calls` binds an own member
/// through its SelfMethod branch (rule `self_method`), a base's member
/// through [`CppScope::resolve`] (rule `implicit_this`). The class's own name
/// (`Cart(...)` inside `Cart`, a construction) is never rewritten. A bound
/// out-of-line member sits under its class after LB.10c, so it takes the
/// same path. Returns the number rewritten; order-preserving.
pub(crate) fn implicit_this(g: &RepoGraph, scope: &CppScope, calls: &mut [CallSite]) -> usize {
    let nav = &g.nav;
    let mut rewritten = 0usize;
    for site in calls.iter_mut() {
        let CallQualifier::Bare(name) = &site.qualifier else {
            continue;
        };
        if nav.kind_by_id.get(&site.from) != Some(&node_kind::METHOD) {
            continue;
        }
        let Some(owner) = enclosing_class_or_struct(nav, site.from) else {
            continue;
        };
        if nav.name_by_id.get(&owner) == Some(name) {
            continue;
        }
        let own = g
            .symbols
            .class_methods
            .get(&owner)
            .is_some_and(|m| m.contains_key(name));
        if own || matches!(scope.base_member(g, owner, name), Found::One(_)) {
            site.qualifier = CallQualifier::SelfMethod(name.clone());
            rewritten += 1;
        }
    }
    rewritten
}

/// Insert `ns` and each of its outer namespaces (`a::b` -> `a::b`, `a`);
/// the global namespace is implicit.
fn add_namespace(set: &mut BTreeSet<String>, ns: &str) {
    let mut cur = ns;
    while !cur.is_empty() && set.insert(cur.to_string()) {
        cur = cur.rsplit_once("::").map_or("", |(outer, _)| outer);
    }
}

/// The directory of a MODULE: its qname minus the file segment
/// (`src::codec.c` -> `src`, a repo-root file -> `""`).
fn module_dir(nav: &CodeNav, module: NodeId) -> String {
    nav.qname_by_id
        .get(&module)
        .and_then(|q| q.rsplit_once("::"))
        .map_or_else(String::new, |(dir, _)| dir.to_string())
}

/// True for a node whose nav parent is a MODULE or a namespace PACKAGE.
fn at_namespace_scope(nav: &CodeNav, id: NodeId) -> bool {
    nav.parent_of
        .get(&id)
        .and_then(|p| nav.kind_by_id.get(p))
        .is_some_and(|k| *k == node_kind::MODULE || *k == node_kind::PACKAGE)
}

/// True when `module`'s file is a translation unit ([`SOURCE_EXTS`]): its
/// qname's last segment is the file name.
fn is_source(nav: &CodeNav, module: NodeId) -> bool {
    nav.qname_by_id
        .get(&module)
        .and_then(|q| q.rsplit("::").next())
        .and_then(|file| file.rsplit_once('.'))
        .is_some_and(|(_, ext)| SOURCE_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// The C++ scope a definition belongs to and its body looks names up from:
/// the C++ name of its nearest CLASS / STRUCT / PACKAGE parent (a METHOD:
/// its class, `shop::Cart`), joined with the qualifier a namespace-qualified
/// definition keeps in its qname (`void util::late() {}` is FUNCTION
/// `<file>::util::late` named `late` under its file: `util`). A file-scope
/// function: `""`.
fn def_scope(nav: &CodeNav, id: NodeId) -> String {
    let Some(&parent) = nav.parent_of.get(&id) else {
        return String::new();
    };
    let scoped =
        is_cpp_type(nav, parent) || nav.kind_by_id.get(&parent) == Some(&node_kind::PACKAGE);
    let base = if scoped {
        cpp_name(nav, parent)
    } else {
        String::new()
    };
    let middle = (|| {
        let rest = nav
            .qname_by_id
            .get(&id)?
            .strip_prefix(nav.qname_by_id.get(&parent)?.as_str())?
            .strip_prefix("::")?;
        rest.strip_suffix(nav.name_by_id.get(&id)?.as_str())?
            .strip_suffix("::")
    })();
    match middle {
        Some(q) if !q.is_empty() => cpp_join(&base, q),
        _ => base,
    }
}

/// True when `base` names a local of `from` or a field of its class: a
/// variable, so `base.m()` is never read as `Type::m()`.
fn names_a_variable(g: &RepoGraph, from: NodeId, base: &str) -> bool {
    let nav = &g.nav;
    nav.local_types
        .get(&from)
        .is_some_and(|locals| locals.contains_key(base))
        || enclosing_class_or_struct(nav, from)
            .and_then(|owner| nav.field_types.get(&owner))
            .is_some_and(|fields| fields.contains_key(base))
}

#[cfg(test)]
mod tests {
    use glia_code_domain::{FileParse, GRAPH_TYPE, ImportStmt, ImportTarget};
    use glia_core::{Confidence, Edge, Node, NodeKindId};

    use super::*;
    use crate::build::build_c_cpp;
    use crate::test_support::repo;

    fn gid(kind: NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    /// One C/C++ file shaped the way the parser emits it: MODULE `module`
    /// (`src::cart.cpp`) and its items, each with a DEFINES edge from its
    /// parent, plus includes, file facts, locals, fields and call sites.
    struct File {
        parse: FileParse,
        module: String,
        ids: HashMap<String, NodeId>,
    }

    impl File {
        fn new(module: &str) -> Self {
            let m = gid(node_kind::MODULE, module);
            let mut parse = FileParse::default();
            let file = module.rsplit("::").next().unwrap_or(module);
            let stem = file.split('.').next().unwrap_or(file);
            parse.nav.record(m, stem, module, node_kind::MODULE, None);
            parse.nodes.push(Node {
                id: m,
                repo: repo(),
                confidence: Confidence::Strong,
                cells: vec![],
            });
            File {
                parse,
                module: module.to_string(),
                ids: HashMap::new(),
            }
        }

        /// An item `(kind, qname, nav name)` under `parent` (an earlier
        /// item's qname; `None`: the MODULE).
        fn item(mut self, kind: NodeKindId, qname: &str, name: &str, parent: Option<&str>) -> Self {
            let id = gid(kind, qname);
            let parent_id = parent.map_or(gid(node_kind::MODULE, &self.module), |p| self.ids[p]);
            self.parse
                .nav
                .record(id, name, qname, kind, Some(parent_id));
            self.parse.nodes.push(Node {
                id,
                repo: repo(),
                confidence: Confidence::Strong,
                cells: vec![],
            });
            self.parse.edges.push(Edge::new(
                parent_id,
                id,
                edge_category::DEFINES,
                Confidence::Strong,
            ));
            self.ids.insert(qname.to_string(), id);
            self
        }

        fn include(mut self, spec: &str) -> Self {
            self.parse.imports.push(ImportStmt {
                from_module: self.module.clone(),
                target: ImportTarget::Module {
                    path: spec.to_string(),
                    alias: None,
                },
                line: 0,
            });
            self
        }

        fn fact(mut self, fact: NavFact) -> Self {
            let m = gid(node_kind::MODULE, &self.module);
            self.parse.nav.record_fact(m, fact);
            self
        }

        fn local(mut self, scope: &str, name: &str, ty: &str) -> Self {
            let id = self.ids[scope];
            self.parse.nav.record_local_type(id, name, ty);
            self
        }

        fn field(mut self, owner: &str, name: &str, ty: &str) -> Self {
            let id = self.ids[owner];
            self.parse.nav.record_field_type(id, name, ty);
            self
        }

        fn call(mut self, from: &str, qualifier: CallQualifier) -> Self {
            let line = u32::try_from(self.parse.calls.len()).unwrap_or(0);
            self.parse.calls.push(CallSite {
                from: self.ids[from],
                qualifier,
                line,
            });
            self
        }
    }

    fn bare(name: &str) -> CallQualifier {
        CallQualifier::Bare(name.into())
    }

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute {
            base: base.into(),
            name: name.into(),
        }
    }

    fn declares(ns: &str, name: &str) -> NavFact {
        NavFact::DeclaresFn {
            ns: ns.into(),
            name: name.into(),
        }
    }

    /// The engine's include resolver, reduced: `spec` relative to the
    /// including file's directory.
    fn include_source(from: &str, spec: &str) -> Option<String> {
        let mut segs: Vec<&str> = from.split("::").collect();
        segs.pop();
        segs.extend(spec.split('/').filter(|p| !p.is_empty() && *p != "."));
        Some(segs.join("::"))
    }

    fn build(files: Vec<File>) -> RepoGraph {
        build_c_cpp(
            repo(),
            files.into_iter().map(|f| f.parse).collect(),
            include_source,
        )
        .expect("builds")
    }

    /// The CALLS edges out of `from` (a qname of `kind`): target qname,
    /// evidence emitter and rule.
    fn calls_from(g: &RepoGraph, kind: NodeKindId, from: &str) -> Vec<(String, String, String)> {
        let from = gid(kind, from);
        g.edges
            .iter()
            .filter(|e| e.from == from && e.category == edge_category::CALLS)
            .map(|e| {
                let to = g.nav.qname_by_id.get(&e.to).cloned().unwrap_or_default();
                let ev = Evidence::of(e).expect("every CALLS edge carries EVIDENCE");
                (to, ev.emitter, ev.rule.unwrap_or_default())
            })
            .collect()
    }

    fn edge(to: &str, emitter: &str, rule: &str) -> (String, String, String) {
        (to.into(), emitter.into(), rule.into())
    }

    /// `src::cart.hpp`: `namespace shop { class Cart { add; clear; reset;
    /// static count; }; }` plus a file-scope free `reset` the generic pass
    /// would otherwise bind.
    fn cart_hpp() -> File {
        File::new("src::cart.hpp")
            .item(node_kind::PACKAGE, "src::cart.hpp::shop", "shop", None)
            .item(
                node_kind::CLASS,
                "shop::Cart",
                "Cart",
                Some("src::cart.hpp::shop"),
            )
            .item(
                node_kind::METHOD,
                "shop::Cart::clear",
                "clear",
                Some("shop::Cart"),
            )
            .item(
                node_kind::METHOD,
                "shop::Cart::reset",
                "reset",
                Some("shop::Cart"),
            )
            .item(node_kind::FUNCTION, "src::cart.hpp::reset", "reset", None)
    }

    /// C5: a bare call inside an inline member names its class's member
    /// before the header's same-named free function (HEAD bound the free
    /// function through the file's symbols).
    #[test]
    fn implicit_this_inline_member() {
        let g = build(vec![cart_hpp().call("shop::Cart::clear", bare("reset"))]);
        assert_eq!(
            calls_from(&g, node_kind::METHOD, "shop::Cart::clear"),
            [edge("shop::Cart::reset", "graph:calls", "self_method")]
        );
    }

    /// C5: an out-of-line member LB.10c joined to its header class takes the
    /// same path: its defining file's static `reset` loses to the member.
    #[test]
    fn implicit_this_out_of_line_member() {
        let cpp = File::new("src::cart.cpp")
            .include("cart.hpp")
            .item(node_kind::FUNCTION, "src::cart.cpp::reset", "reset", None)
            .item(node_kind::PACKAGE, "src::cart.cpp::shop", "shop", None)
            .item(
                node_kind::METHOD,
                "shop::Cart::add",
                "Cart::add",
                Some("src::cart.cpp::shop"),
            )
            .call("shop::Cart::add", bare("reset"));
        let g = build(vec![cart_hpp(), cpp]);
        let add = gid(node_kind::METHOD, "shop::Cart::add");
        assert_eq!(g.nav.parent_of[&add], gid(node_kind::CLASS, "shop::Cart"));
        assert_eq!(
            calls_from(&g, node_kind::METHOD, "shop::Cart::add"),
            [edge("shop::Cart::reset", "graph:calls", "self_method")]
        );
    }

    /// C5: a member only a base declares binds through the INHERITS_FROM
    /// edge, before the file's free function, rule `implicit_this`; the
    /// class's own name (a construction) is never rewritten.
    #[test]
    fn implicit_this_through_a_base() {
        let mut h = File::new("src::w.hpp")
            .item(node_kind::CLASS, "src::Base", "Base", None)
            .item(
                node_kind::METHOD,
                "src::Base::ping",
                "ping",
                Some("src::Base"),
            )
            .item(node_kind::CLASS, "src::Derived", "Derived", None)
            .item(
                node_kind::METHOD,
                "src::Derived::Derived",
                "Derived",
                Some("src::Derived"),
            )
            .item(
                node_kind::METHOD,
                "src::Derived::run",
                "run",
                Some("src::Derived"),
            )
            .item(node_kind::FUNCTION, "src::w.hpp::ping", "ping", None)
            .call("src::Derived::run", bare("ping"))
            .call("src::Derived::run", bare("Derived"));
        h.parse.edges.push(Edge::new(
            gid(node_kind::CLASS, "src::Derived"),
            gid(node_kind::CLASS, "src::Base"),
            edge_category::INHERITS_FROM,
            Confidence::Strong,
        ));
        let mut calls = h.parse.calls.clone();
        let g = build(vec![h]);
        // `Derived(...)` stays a construction, bound as before to the class.
        assert_eq!(
            calls_from(&g, node_kind::METHOD, "src::Derived::run"),
            [
                edge("src::Base::ping", EMITTER, IMPLICIT_THIS),
                edge("src::Derived", "graph:calls", "module_symbol")
            ]
        );
        let scope = CppScope::new(&g, &HashMap::new());
        assert_eq!(implicit_this(&g, &scope, &mut calls), 1);
        assert_eq!(calls[0].qualifier, CallQualifier::SelfMethod("ping".into()));
        assert_eq!(calls[1].qualifier, bare("Derived"));
    }

    /// C5: `Cart::count()` from a member names the class through C++ lookup
    /// from the caller's scope (`shop::Cart` -> `shop`), so the included
    /// `legacy::Cart::count` never answers.
    #[test]
    fn static_type_call() {
        let legacy = File::new("src::legacy.hpp")
            .item(
                node_kind::PACKAGE,
                "src::legacy.hpp::legacy",
                "legacy",
                None,
            )
            .item(
                node_kind::CLASS,
                "legacy::Cart",
                "Cart",
                Some("src::legacy.hpp::legacy"),
            )
            .item(
                node_kind::METHOD,
                "legacy::Cart::count",
                "count",
                Some("legacy::Cart"),
            );
        let hpp = cart_hpp().item(
            node_kind::METHOD,
            "shop::Cart::count",
            "count",
            Some("shop::Cart"),
        );
        let cpp = File::new("src::cart.cpp")
            .include("cart.hpp")
            .include("legacy.hpp")
            .item(node_kind::PACKAGE, "src::cart.cpp::shop", "shop", None)
            .item(
                node_kind::METHOD,
                "shop::Cart::add",
                "Cart::add",
                Some("src::cart.cpp::shop"),
            )
            .call("shop::Cart::add", attr("Cart", "count"))
            .call("shop::Cart::add", attr("legacy::Cart", "count"))
            .item(node_kind::FUNCTION, "src::cart.cpp::run", "run", None)
            .call("src::cart.cpp::run", attr("Cart", "count"));
        let g = build(vec![hpp, legacy, cpp]);
        assert_eq!(
            calls_from(&g, node_kind::METHOD, "shop::Cart::add"),
            [
                edge("shop::Cart::count", EMITTER, QUALIFIED),
                edge("legacy::Cart::count", EMITTER, QUALIFIED)
            ]
        );
        // From the global namespace, with no `using`, `Cart` names nothing.
        assert!(calls_from(&g, node_kind::FUNCTION, "src::cart.cpp::run").is_empty());
    }

    /// C5: `util::clamp()` binds the function of the namespace a visible
    /// file (an include of an include) declares; `other::clamp` and a
    /// namespace no visible file defines the function in do not answer.
    #[test]
    fn namespace_qualified_function() {
        let util = File::new("src::util.hpp")
            .item(node_kind::PACKAGE, "src::util.hpp::util", "util", None)
            .item(
                node_kind::FUNCTION,
                "src::util.hpp::util::clamp",
                "clamp",
                Some("src::util.hpp::util"),
            );
        let other = File::new("src::other.hpp")
            .item(node_kind::PACKAGE, "src::other.hpp::other", "other", None)
            .item(
                node_kind::FUNCTION,
                "src::other.hpp::other::clamp",
                "clamp",
                Some("src::other.hpp::other"),
            );
        let mid = File::new("src::mid.hpp").include("util.hpp");
        let main = File::new("src::main.cpp")
            .include("mid.hpp")
            .item(node_kind::FUNCTION, "src::main.cpp::main", "main", None)
            .call("src::main.cpp::main", attr("util", "clamp"))
            .call("src::main.cpp::main", attr("::util", "clamp"))
            .call("src::main.cpp::main", attr("other", "clamp"));
        let g = build(vec![util, other, mid, main]);
        assert_eq!(
            calls_from(&g, node_kind::FUNCTION, "src::main.cpp::main"),
            [
                edge("src::util.hpp::util::clamp", EMITTER, QUALIFIED),
                edge("src::util.hpp::util::clamp", EMITTER, QUALIFIED)
            ]
        );
    }

    /// C7: with `shop::Cart` and `legacy::Cart` both included, a local
    /// `Cart c` under `using namespace shop;` is `shop::Cart`, and under
    /// `using legacy::Cart;` is `legacy::Cart` (a using-declaration before
    /// the directives); with neither, the short name is refused.
    #[test]
    fn using_namespace_picks_the_right_type() {
        let shape = |facts: Vec<NavFact>| {
            let shop = File::new("src::shop.hpp")
                .item(node_kind::PACKAGE, "src::shop.hpp::shop", "shop", None)
                .item(
                    node_kind::CLASS,
                    "shop::Cart",
                    "Cart",
                    Some("src::shop.hpp::shop"),
                )
                .item(
                    node_kind::METHOD,
                    "shop::Cart::add",
                    "add",
                    Some("shop::Cart"),
                );
            let legacy = File::new("src::legacy.hpp")
                .item(
                    node_kind::PACKAGE,
                    "src::legacy.hpp::legacy",
                    "legacy",
                    None,
                )
                .item(
                    node_kind::CLASS,
                    "legacy::Cart",
                    "Cart",
                    Some("src::legacy.hpp::legacy"),
                )
                .item(
                    node_kind::METHOD,
                    "legacy::Cart::add",
                    "add",
                    Some("legacy::Cart"),
                );
            let mut cpp = File::new("src::cart.cpp")
                .include("shop.hpp")
                .include("legacy.hpp")
                .item(node_kind::FUNCTION, "src::cart.cpp::run", "run", None)
                .local("src::cart.cpp::run", "c", "Cart")
                .call("src::cart.cpp::run", attr("c", "add"));
            for f in facts {
                cpp = cpp.fact(f);
            }
            build(vec![shop, legacy, cpp])
        };
        let run = |g: &RepoGraph| calls_from(g, node_kind::FUNCTION, "src::cart.cpp::run");
        let directive = NavFact::UsingNamespace {
            within: String::new(),
            ns: "shop".into(),
        };
        assert_eq!(
            run(&shape(vec![directive.clone()])),
            [edge("shop::Cart::add", EMITTER, RECEIVER_USING)]
        );
        let declaration = NavFact::UsingName {
            within: String::new(),
            ns: "legacy".into(),
            name: "Cart".into(),
        };
        assert_eq!(
            run(&shape(vec![directive, declaration])),
            [edge("legacy::Cart::add", EMITTER, RECEIVER_USING)]
        );
        assert!(run(&shape(vec![])).is_empty());
        // Both namespaces pulled in: ambiguous, refused.
        let both = shape(vec![
            NavFact::UsingNamespace {
                within: String::new(),
                ns: "shop".into(),
            },
            NavFact::UsingNamespace {
                within: String::new(),
                ns: "legacy".into(),
            },
        ]);
        assert!(run(&both).is_empty());
    }

    /// `src::codec.h` declares `codec_encode`; `src::codec.c` defines it and
    /// a `static shift`; `src::cart.hpp` includes the header and
    /// `src::cart.cpp` includes `cart.hpp` (an include of an include) and
    /// `includes`; `extra` files join the build.
    fn codec_shape(includes: &[&str], extra: Vec<File>) -> Vec<File> {
        let h = File::new("src::codec.h").fact(declares("", "codec_encode"));
        let c = File::new("src::codec.c")
            .include("codec.h")
            .fact(NavFact::InternalLinkage {
                name: "shift".into(),
            })
            .item(node_kind::FUNCTION, "src::codec.c::shift", "shift", None)
            .item(
                node_kind::FUNCTION,
                "src::codec.c::codec_encode",
                "codec_encode",
                None,
            );
        let hpp = File::new("src::cart.hpp").include("codec.h");
        let mut cpp = File::new("src::cart.cpp").include("cart.hpp");
        for spec in includes {
            cpp = cpp.include(spec);
        }
        let cpp = cpp
            .item(node_kind::FUNCTION, "src::cart.cpp::run", "run", None)
            .call("src::cart.cpp::run", bare("codec_encode"))
            .call("src::cart.cpp::run", bare("shift"));
        let mut files = vec![h, c, hpp, cpp];
        files.extend(extra);
        files
    }

    /// C6: a call through a prototype two includes away binds the one
    /// external definition; the prototype mints no node of its own.
    #[test]
    fn prototype_through_include_of_include() {
        let g = build(codec_shape(&[], vec![]));
        assert_eq!(
            calls_from(&g, node_kind::FUNCTION, "src::cart.cpp::run"),
            [edge("src::codec.c::codec_encode", EMITTER, PROTOTYPE)]
        );
    }

    /// C6: a `static` definition is never a prototype's target, nor is a
    /// definition in a header the caller does not include: with `shift`
    /// declared in a visible `shift.h`, only a definition with external
    /// linkage in a source file answers.
    #[test]
    fn static_definition_is_never_the_prototype_target() {
        let shift_h = || File::new("src::shift.h").fact(declares("", "shift"));
        let in_header = File::new("src::shift_inline.h").item(
            node_kind::FUNCTION,
            "src::shift_inline.h::shift",
            "shift",
            None,
        );
        let g = build(codec_shape(&["shift.h"], vec![shift_h(), in_header]));
        assert_eq!(
            calls_from(&g, node_kind::FUNCTION, "src::cart.cpp::run"),
            [edge("src::codec.c::codec_encode", EMITTER, PROTOTYPE)]
        );
        // An external definition elsewhere is the one that binds.
        let shift_c = File::new("src::shift.c").item(
            node_kind::FUNCTION,
            "src::shift.c::shift",
            "shift",
            None,
        );
        let g = build(codec_shape(&["shift.h"], vec![shift_h(), shift_c]));
        assert_eq!(
            calls_from(&g, node_kind::FUNCTION, "src::cart.cpp::run"),
            [
                edge("src::codec.c::codec_encode", EMITTER, PROTOTYPE),
                edge("src::shift.c::shift", EMITTER, PROTOTYPE)
            ]
        );
    }

    /// C6: two external definitions of one prototype beside its header (a
    /// file per platform) are refused and counted ambiguous.
    #[test]
    fn two_external_definitions_are_ambiguous() {
        let other = File::new("src::codec_win.c").item(
            node_kind::FUNCTION,
            "src::codec_win.c::codec_encode",
            "codec_encode",
            None,
        );
        let files = codec_shape(&[], vec![other]);
        let mut calls: Vec<CallSite> = files[3].parse.calls.clone();
        let g = build(files);
        assert!(calls_from(&g, node_kind::FUNCTION, "src::cart.cpp::run").is_empty());
        let scope = CppScope::new(&g, &HashMap::new());
        let rewritten = implicit_this(&g, &scope, &mut calls);
        for site in &calls {
            assert!(scope.resolve(&g, site).is_none(), "{site:?}");
        }
        assert_eq!(
            scope.marker(rewritten),
            "[cpp-scope] implicit_this=0 qualified=0 receiver_using=0 prototype=0 using=0 ambiguous=1"
        );
    }

    /// C6: two programs of one repo each define `GetArgs` beside their own
    /// `utils.h`: a caller binds the definition in the directory of the
    /// header it includes, never the other program's.
    #[test]
    fn several_definitions_bind_the_one_beside_the_header() {
        let program = |dir: &str| {
            let (h, c, main) = (
                format!("{dir}::utils.h"),
                format!("{dir}::utils.cpp"),
                format!("{dir}::main.cpp"),
            );
            vec![
                File::new(&h).fact(declares("", "GetArgs")),
                File::new(&c).include("utils.h").item(
                    node_kind::FUNCTION,
                    &format!("{c}::GetArgs"),
                    "GetArgs",
                    None,
                ),
                File::new(&main)
                    .include("utils.h")
                    .item(node_kind::FUNCTION, &format!("{main}::main"), "main", None)
                    .call(&format!("{main}::main"), bare("GetArgs")),
            ]
        };
        let mut files = program("a::runner");
        files.extend(program("b::runner"));
        let g = build(files);
        for dir in ["a::runner", "b::runner"] {
            assert_eq!(
                calls_from(&g, node_kind::FUNCTION, &format!("{dir}::main.cpp::main")),
                [edge(
                    &format!("{dir}::utils.cpp::GetArgs"),
                    EMITTER,
                    PROTOTYPE
                )]
            );
        }
    }

    /// C7: a bare call through `using util::scale;` / `using namespace
    /// util;` binds the namespace's function (a visible definition, or the
    /// external definition of its visible prototype); a directive written
    /// inside another namespace does not apply at file scope.
    #[test]
    fn bare_call_through_using() {
        let shape = |fact: NavFact| {
            let main = File::new("src::main.cpp")
                .include("util.hpp")
                .fact(fact)
                .item(node_kind::FUNCTION, "src::main.cpp::main", "main", None)
                .call("src::main.cpp::main", bare("clamp"))
                .call("src::main.cpp::main", bare("scale"));
            let (h, c) = (
                File::new("src::util.hpp")
                    .fact(declares("util", "scale"))
                    .item(node_kind::PACKAGE, "src::util.hpp::util", "util", None)
                    .item(
                        node_kind::FUNCTION,
                        "src::util.hpp::util::clamp",
                        "clamp",
                        Some("src::util.hpp::util"),
                    ),
                File::new("src::util.cpp")
                    .include("util.hpp")
                    .item(node_kind::PACKAGE, "src::util.cpp::util", "util", None)
                    .item(
                        node_kind::FUNCTION,
                        "src::util.cpp::util::scale",
                        "scale",
                        Some("src::util.cpp::util"),
                    ),
            );
            build(vec![h, c, main])
        };
        let main = |g: &RepoGraph| calls_from(g, node_kind::FUNCTION, "src::main.cpp::main");
        assert_eq!(
            main(&shape(NavFact::UsingNamespace {
                within: String::new(),
                ns: "util".into()
            })),
            [
                edge("src::util.hpp::util::clamp", EMITTER, USING),
                edge("src::util.cpp::util::scale", EMITTER, USING)
            ]
        );
        assert_eq!(
            main(&shape(NavFact::UsingName {
                within: String::new(),
                ns: "util".into(),
                name: "scale".into()
            })),
            [edge("src::util.cpp::util::scale", EMITTER, USING)]
        );
        assert!(
            main(&shape(NavFact::UsingNamespace {
                within: "app".into(),
                ns: "util".into()
            }))
            .is_empty()
        );
    }

    /// C6 / C7 lookups only see what the include graph reaches: a prototype
    /// in a header the caller never includes binds nothing.
    #[test]
    fn a_prototype_the_caller_does_not_include_is_not_in_scope() {
        let mut files = codec_shape(&[], vec![]);
        files[2] = File::new("src::cart.hpp");
        let g = build(files);
        assert!(calls_from(&g, node_kind::FUNCTION, "src::cart.cpp::run").is_empty());
    }

    /// The pass is deterministic: two builds of one shape give the same
    /// edges in the same order.
    #[test]
    fn builds_are_identical() {
        let edges = |g: RepoGraph| {
            g.edges
                .iter()
                .map(|e| (e.from.0, e.to.0, e.category))
                .collect::<Vec<_>>()
        };
        let a = edges(build(codec_shape(&[], vec![])));
        let b = edges(build(codec_shape(&[], vec![])));
        assert_eq!(a, b);
    }

    /// C5 / C7: a field typed by a short name two namespaces share
    /// (`shop::Box`, `legacy::Box`) is looked up from its class's scope, so
    /// `box.get()` in a `shop::Cart` member is `shop::Box::get`.
    #[test]
    fn field_receiver_resolves_from_the_class_scope() {
        let boxes = File::new("src::box.hpp")
            .item(node_kind::PACKAGE, "src::box.hpp::shop", "shop", None)
            .item(
                node_kind::CLASS,
                "shop::Box",
                "Box",
                Some("src::box.hpp::shop"),
            )
            .item(
                node_kind::METHOD,
                "shop::Box::get",
                "get",
                Some("shop::Box"),
            )
            .item(node_kind::PACKAGE, "src::box.hpp::legacy", "legacy", None)
            .item(
                node_kind::CLASS,
                "legacy::Box",
                "Box",
                Some("src::box.hpp::legacy"),
            )
            .item(
                node_kind::METHOD,
                "legacy::Box::get",
                "get",
                Some("legacy::Box"),
            );
        let cart = cart_hpp()
            .include("box.hpp")
            .field("shop::Cart", "box", "Box")
            .call("shop::Cart::clear", attr("box", "get"));
        let g = build(vec![boxes, cart]);
        assert_eq!(
            calls_from(&g, node_kind::METHOD, "shop::Cart::clear"),
            [edge("shop::Box::get", EMITTER, RECEIVER_USING)]
        );
    }

    #[test]
    fn def_scope_reads_classes_namespaces_and_qualified_definitions() {
        let g = build(vec![
            cart_hpp(),
            File::new("src::util.cpp")
                .item(
                    node_kind::FUNCTION,
                    "src::util.cpp::util::late",
                    "late",
                    None,
                )
                .item(node_kind::FUNCTION, "src::util.cpp::top", "top", None),
        ]);
        let nav = &g.nav;
        assert_eq!(
            def_scope(nav, gid(node_kind::METHOD, "shop::Cart::clear")),
            "shop::Cart"
        );
        assert_eq!(def_scope(nav, gid(node_kind::CLASS, "shop::Cart")), "shop");
        assert_eq!(
            def_scope(nav, gid(node_kind::FUNCTION, "src::util.cpp::util::late")),
            "util"
        );
        assert_eq!(
            def_scope(nav, gid(node_kind::FUNCTION, "src::util.cpp::top")),
            ""
        );
    }
}
