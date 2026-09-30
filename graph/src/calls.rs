//! Call / ref resolution and the nav-walking helpers it needs.

use std::collections::HashMap;

use glia_code_domain::evidence::{self, Evidence};
use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, UnresolvedRef, cell_type, edge_category, node_kind,
    recv_stats,
};
use glia_core::{Confidence, Edge, EdgeCategoryId, NodeId};

use crate::types::RepoGraph;

// ============================================================================
// Resolution evidence (LC.3d)
// ============================================================================

/// The evidence a graph-resolved edge carries: the mechanism that bound it
/// (`graph:calls`, `graph:refs`, `graph:imports`, `graph:iface`,
/// `graph:rust_paths`, `graph:go_packages`, `graph:nav`) and the branch
/// inside it. No location: an edge resolved from a `CallSite` /
/// `ImportStmt` / `UnresolvedRef` adds the site's line (LC.3b,
/// [`Evidence::line`]); the engine's fill pass places the rest from the
/// edge's endpoints.
pub(crate) fn graph_evidence(emitter: &str, rule: &str) -> Evidence {
    Evidence::emitter(emitter).rule(rule)
}

/// Node id -> its file, as the engine's fill pass locates it
/// ([`evidence::locate`]), for the site evidence of edges `resolve_refs`
/// binds (LC.3b). A ref's `from` may carry no location (a nav ROUTE, a
/// CRON_JOB, a CLI_COMMAND) or several (a Go ROUTE registered in two files);
/// its `from_module` is the file the reference was read in.
pub(crate) struct SiteFiles(HashMap<NodeId, String>);

impl SiteFiles {
    pub(crate) fn of(g: &RepoGraph) -> Self {
        let mut files: HashMap<NodeId, String> = HashMap::new();
        for n in &g.nodes {
            if files.contains_key(&n.id) {
                continue;
            }
            if let Some((file, _)) = evidence::locate(&n.cells) {
                files.insert(n.id, file);
            }
        }
        SiteFiles(files)
    }

    /// `ev` at 0-based `line` of `r`'s file: its `from_module`'s, else its
    /// `from`'s; with neither located, the line alone (the fill pass then
    /// takes the file from the edge's `from`).
    pub(crate) fn place(&self, ev: Evidence, r: &UnresolvedRef) -> Evidence {
        match self.0.get(&r.from_module).or_else(|| self.0.get(&r.from)) {
            Some(file) => ev.at(file.clone(), r.line),
            None => ev.line(r.line),
        }
    }
}

/// The branch of [`resolve_calls`] / [`resolve_refs`] that bound a site,
/// written as the EVIDENCE `rule` of the edge it draws. The name-only
/// fallbacks (`global_unique`, `global_unique_method`) are the edges a
/// reviewer discounts.
#[derive(Clone, Copy)]
enum Branch {
    ImportBinding,
    ModuleSymbol,
    PackageSymbol,
    Attribute,
    SelfMethod,
    ReceiverType,
    GlobalUnique,
    EnumMember,
    GlobalUniqueMethod,
    TypeMethod,
}

impl Branch {
    const COUNT: usize = 10;

    fn rule(self) -> &'static str {
        match self {
            Branch::ImportBinding => "import_binding",
            Branch::ModuleSymbol => "module_symbol",
            Branch::PackageSymbol => "package_symbol",
            Branch::Attribute => "attribute",
            Branch::SelfMethod => "self_method",
            Branch::ReceiverType => "receiver_type",
            Branch::GlobalUnique => "global_unique",
            Branch::EnumMember => "enum_member",
            Branch::GlobalUniqueMethod => "global_unique_method",
            Branch::TypeMethod => "type_method",
        }
    }
}

/// One graph build's `[evidence-graph]` tallies: the edges `resolve_calls`
/// drew per branch (a hit of its `extra_hook` counts `extra_hook`, whatever
/// rule the hook named) and the name-only / enum-member binds of
/// `resolve_refs`. A builder that runs `resolve_calls` twice (Go) sums both.
#[derive(Default)]
pub(crate) struct EvidenceTally {
    calls: [usize; Branch::COUNT],
    extra_hook: usize,
    refs: [usize; Branch::COUNT],
}

impl EvidenceTally {
    /// LC.3d fired_on marker, fixed order, `None` when no call resolved:
    /// `[evidence-graph] calls import_binding=a module_symbol=b package_symbol=c
    /// attribute=d self_method=e receiver_type=f extra_hook=g refs
    /// global_unique=h enum_member=i`.
    fn marker(&self) -> Option<String> {
        let c = |b: Branch| self.calls[b as usize];
        let resolved: usize = self.calls.iter().sum::<usize>() + self.extra_hook;
        (resolved > 0).then(|| {
            format!(
                "[evidence-graph] calls import_binding={} module_symbol={} package_symbol={} \
                 attribute={} self_method={} receiver_type={} extra_hook={} refs global_unique={} \
                 enum_member={}",
                c(Branch::ImportBinding),
                c(Branch::ModuleSymbol),
                c(Branch::PackageSymbol),
                c(Branch::Attribute),
                c(Branch::SelfMethod),
                c(Branch::ReceiverType),
                self.extra_hook,
                self.refs[Branch::GlobalUnique as usize],
                self.refs[Branch::EnumMember as usize],
            )
        })
    }

    /// Print the marker, once per graph build, after its last resolve pass.
    pub(crate) fn report(&self) {
        if let Some(line) = self.marker() {
            eprintln!("{line}");
        }
    }
}

// ============================================================================
// Call resolution
// ============================================================================

/// Cross-file call resolution — same recipe for all languages.
///
/// `extra_hook` is an escape hatch for language-specific resolution shapes
/// that the generic pass doesn't cover, consulted only after every generic
/// lookup misses. Rust passes its path resolver (`crate::rust_paths`, LA.1a)
/// and Go its package-directory hook (`build::GoPackages`, LA.13b); every
/// other builder passes `|_, _| None`. A hook hit carries the hook's own
/// evidence (its mechanism and branch); every generic hit is `graph:calls`
/// with the [`Branch`] that bound it.
pub(crate) fn resolve_calls<H>(
    g: &mut RepoGraph,
    calls: &[CallSite],
    extra_hook: H,
    tally: &mut EvidenceTally,
) where
    H: Fn(&RepoGraph, &CallSite) -> Option<(NodeId, Evidence)>,
{
    let mut pkg_base_bound = 0usize;
    let mut enum_hits = EnumHits::default();
    let mut gate = BareFieldGate::default();
    // A6.6: CALLS bound into an INTERFACE's own METHOD, by the path that bound
    // them (a field-typed receiver, or a statically-qualified `IFoo.m()`).
    let (mut iface_recv, mut iface_static) = (0usize, 0usize);
    for site in calls {
        // CB.15: the caller's OWN file, also for a member of a namespace
        // several files open; `enclosing_package` below still walks the nav.
        let Some(from_module) = enclosing_home_module(g, site.from) else {
            g.unresolved_calls.push(site.clone());
            continue;
        };
        let bindings = g.symbols.module_import_bindings.get(&from_module);

        let resolved: Option<(NodeId, Branch)> = match &site.qualifier {
            CallQualifier::Bare(name) => {
                bare_call_target(g, bindings, from_module, site.from, name)
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
                        Some((_, k)) if k == node_kind::INTERFACE => iface_static += 1,
                        _ => {}
                    }
                }
                hit.map(|to| (to, Branch::Attribute))
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
                hit.map(|to| (to, Branch::SelfMethod))
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
            let hit = resolve_via_receiver_type(g, site, from_module, &mut gate);
            if let Some(to) = hit {
                recv_stats::record();
                if is_interface_method(&g.nav, to) {
                    iface_recv += 1;
                }
            }
            hit.map(|to| (to, Branch::ReceiverType))
        });

        let resolved = match resolved {
            Some((to, branch)) => {
                tally.calls[branch as usize] += 1;
                Some((to, graph_evidence("graph:calls", branch.rule())))
            }
            None => extra_hook(g, site).inspect(|_| tally.extra_hook += 1),
        };

        match resolved {
            // LC.3b: the call expression's own row, basis site.
            Some((to, ev)) => push_edge(g, site.from, to, edge_category::CALLS, ev.line(site.line)),
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
    if iface_recv + iface_static > 0 {
        eprintln!(
            "[iface] interface-method calls bound: {} (receiver={iface_recv} static={iface_static})",
            iface_recv + iface_static
        );
    }
    if let Some(line) = gate.marker() {
        eprintln!("{line}");
    }
}

/// A Bare call's target and the branch that found it, in the fixed priority:
/// the caller module's import binding, its own top-level def, then the
/// enclosing PACKAGE's def (Elixir: `def`s live under a defmodule PACKAGE, not
/// the file MODULE). The first hit wins, so the branch is the one that bound.
fn bare_call_target(
    g: &RepoGraph,
    bindings: Option<&HashMap<String, NodeId>>,
    from_module: NodeId,
    from: NodeId,
    name: &str,
) -> Option<(NodeId, Branch)> {
    if let Some(to) = bindings.and_then(|b| b.get(name).copied()) {
        return Some((to, Branch::ImportBinding));
    }
    let own = |module: NodeId| {
        g.symbols
            .module_symbols
            .get(&module)
            .and_then(|s| s.get(name).copied())
    };
    if let Some(to) = own(from_module) {
        return Some((to, Branch::ModuleSymbol));
    }
    own(enclosing_package(&g.nav, from)?).map(|to| (to, Branch::PackageSymbol))
}

/// True when `id` is a METHOD owned directly by an INTERFACE.
fn is_interface_method(nav: &CodeNav, id: NodeId) -> bool {
    nav.parent_of
        .get(&id)
        .and_then(|p| nav.kind_by_id.get(p))
        == Some(&node_kind::INTERFACE)
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
        lowest_qname_ext(g, &self.enums)
    }
}

/// Per-build tally of interface → super-interface heritage refs (LD.7a):
/// INHERITS_FROM refs whose `from` is an INTERFACE, split by whether
/// `resolve_refs` bound them. `ifaces` feeds the marker's `ext` discriminator —
/// the generic pass does not know which language it is building.
#[derive(Default)]
struct IfaceExtends {
    bound: usize,
    unresolved: usize,
    ifaces: Vec<NodeId>,
}

impl IfaceExtends {
    /// `[heritage] interface-extends bound=N unresolved=M ext=<ext>`, or None
    /// when the build saw no interface heritage ref.
    fn marker(&self, g: &RepoGraph) -> Option<String> {
        if self.bound + self.unresolved == 0 {
            return None;
        }
        Some(format!(
            "[heritage] interface-extends bound={} unresolved={} ext={}",
            self.bound,
            self.unresolved,
            lowest_qname_ext(g, &self.ifaces)
        ))
    }
}

/// An interface -> super-interface heritage ref (LD.7a): INHERITS_FROM out of
/// an INTERFACE. A class's `extends` and any IMPLEMENTS ref are not.
fn extends_interface(g: &RepoGraph, r: &UnresolvedRef) -> bool {
    r.category == edge_category::INHERITS_FROM
        && g.nav.kind_by_id.get(&r.from) == Some(&node_kind::INTERFACE)
}

/// File extension of the POSITION cell of the lowest-qname node in `ids`, or
/// `?` when that node carries no POSITION (or its file has no extension) — a
/// marker's language discriminator.
fn lowest_qname_ext(g: &RepoGraph, ids: &[NodeId]) -> String {
    // NodeId is Hash, not Ord: order by qname alone. Two ids with the same
    // qname are the same node (the id derives from kind + qname).
    let lowest = ids
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

/// The `file` field of a node's POSITION cell (JSON `{"file":"…",…}`). Also
/// read by `nav` to place a linking file under its LB.4a project owner.
pub(crate) fn position_file(node: &glia_core::Node) -> Option<String> {
    node.cells.iter().find_map(|c| {
        if c.kind != cell_type::POSITION {
            return None;
        }
        let glia_core::CellPayload::Json(j) = &c.payload else {
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
///
/// `NAVIGATES_TO` refs (LA.6a) are not symbol references: they name a URL
/// path, so they are split off here and bound against the nav-route table by
/// `nav::resolve_nav_links` after every other ref, then `nav::lift_nav_endpoints`
/// moves file-level page-flow endpoints onto their page component. The
/// partition keeps the other refs in their original order, so every other
/// edge list is unchanged.
pub(crate) fn resolve_refs(g: &mut RepoGraph, refs: &[UnresolvedRef], tally: &mut EvidenceTally) {
    let (nav_refs, refs): (Vec<&UnresolvedRef>, Vec<&UnresolvedRef>) =
        refs.iter().partition(|r| r.category == edge_category::NAVIGATES_TO);
    let files = if refs.is_empty() && nav_refs.is_empty() {
        SiteFiles(HashMap::new())
    } else {
        SiteFiles::of(g)
    };
    let mut pkg_base_bound = 0usize;
    let mut enum_hits = EnumHits::default();
    let mut iface_extends = IfaceExtends::default();
    for r in refs {
        let extends_iface = extends_interface(g, r);
        let bindings = g.symbols.module_import_bindings.get(&r.from_module);
        let resolved: Option<(NodeId, Branch)> = match &r.qualifier {
            CallQualifier::Bare(name) => match bindings.and_then(|b| b.get(name).copied()) {
                // A6.3: a supertype is a type, never a file. A TS default
                // import (`import Base from "./base"`) binds its local name to
                // the MODULE; heritage looks through it to that module's
                // same-named def. The name is import-bound, so a miss stays
                // unresolved instead of falling back to a repo-wide lookup.
                Some(id) if is_heritage(r.category) => {
                    heritage_through_module(g, id, name).map(|to| (to, Branch::ImportBinding))
                }
                Some(id) => Some((id, Branch::ImportBinding)),
                None => g
                    .symbols
                    .module_symbols
                    .get(&r.from_module)
                    .and_then(|s| s.get(name).copied())
                    .map(|to| (to, Branch::ModuleSymbol))
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
                            unique_global_function(g, name).map(|to| (to, Branch::GlobalUnique))
                        } else {
                            None
                        }
                    }),
            },
            CallQualifier::Attribute { base, name } => {
                let bound_base = attribute_base(g, bindings, base);
                let hit = resolve_attribute_target(g, bindings, base, name)
                    .map(|to| (to, Branch::Attribute));
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
                        member.map(|to| (to, Branch::EnumMember))
                    }
                    _ => None,
                });
                // CA.5a: a HANDLED_BY base naming a STRUCT / CLASS the
                // registering module itself declares binds that type's own
                // method. The Go parser writes a receiver method value
                // (`h.List` inside `func (h *TokensHandler) RegisterRoutes`)
                // with the receiver's TYPE as its base, so a second type with
                // a `List` cannot make it ambiguous.
                let hit = hit.or_else(|| {
                    if r.category == edge_category::HANDLED_BY {
                        module_type_method(g, r.from_module, base, name)
                            .map(|to| (to, Branch::TypeMethod))
                    } else {
                        None
                    }
                });
                // Global fallback for HANDLED_BY: in Go, route handlers
                // are usually written `h.GetProfile` where `h` is a local
                // struct-receiver variable (`h *Handlers`), not an import
                // binding. So binding lookup fails. Scan all class_methods
                // across the graph for a method matching `name`; emit
                // only when exactly one match exists. A receiver type
                // declared in another file of its package misses the arm
                // above and lands here.
                hit.or_else(|| {
                    if r.category == edge_category::HANDLED_BY {
                        unique_global_method(g, name).map(|to| (to, Branch::GlobalUniqueMethod))
                    } else {
                        None
                    }
                })
            }
            CallQualifier::SelfMethod(_)
            | CallQualifier::SuperMethod(_)
            | CallQualifier::ComplexReceiver { .. } => None,
        };

        if extends_iface {
            match resolved {
                Some(_) => iface_extends.bound += 1,
                None => iface_extends.unresolved += 1,
            }
            iface_extends.ifaces.push(r.from);
        }
        match resolved {
            Some((to, branch)) => {
                tally.refs[branch as usize] += 1;
                let ev = files.place(graph_evidence("graph:refs", branch.rule()), r);
                push_edge(g, r.from, to, r.category, ev);
            }
            None => g.unresolved_refs.push(r.clone()),
        }
    }
    if pkg_base_bound > 0 {
        eprintln!("[resolve] package-base attribute calls bound: {pkg_base_bound}");
    }
    if enum_hits.uses > 0 {
        eprintln!("[resolve] enum member uses bound: {} ext={}", enum_hits.uses, enum_hits.ext(g));
    }
    // LD.7a fired_on marker, once per language graph build:
    //   `[heritage] interface-extends bound=N unresolved=M ext=<ext>`
    if let Some(line) = iface_extends.marker(g) {
        eprintln!("{line}");
    }
    let mut nav = crate::nav::resolve_nav_links(g, &nav_refs, &files);
    nav.lifted = crate::nav::lift_nav_endpoints(g);
    if nav.fired() {
        eprintln!("{}", nav.marker());
    }
}

fn is_heritage(category: EdgeCategoryId) -> bool {
    category == edge_category::INHERITS_FROM || category == edge_category::IMPLEMENTS
}

/// The heritage target an import binding names (A6.3): the bound node itself,
/// unless it is a MODULE (a TS default import binds the file), in which case
/// the module's own top-level def named `name`, or None.
fn heritage_through_module(g: &RepoGraph, bound: NodeId, name: &str) -> Option<NodeId> {
    if g.nav.kind_by_id.get(&bound) != Some(&node_kind::MODULE) {
        return Some(bound);
    }
    g.symbols.module_symbols.get(&bound).and_then(|s| s.get(name).copied())
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
        // A6.6: a statically-qualified interface call (`IFoo.Of()`, a Java
        // static interface method, a Rust `Trait::f`) binds the interface's
        // own METHOD through its separate table.
        Some(k) if k == node_kind::INTERFACE => {
            g.symbols.interface_methods.get(&base_id).and_then(|m| m.get(name).copied())
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
) -> Option<(NodeId, glia_core::NodeKindId)> {
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

/// CA.5a: the method `name` of the STRUCT / CLASS named `ty` among
/// `module`'s own top-level defs. Only a method OF that type is ever returned,
/// so a same-named method of another type cannot bind; a promoted method (an
/// embedded struct's) is not in `class_methods` and misses.
fn module_type_method(g: &RepoGraph, module: NodeId, ty: &str, name: &str) -> Option<NodeId> {
    let type_id = *g.symbols.module_symbols.get(&module)?.get(ty)?;
    match g.nav.kind_by_id.get(&type_id).copied() {
        Some(k) if k == node_kind::STRUCT || k == node_kind::CLASS => {
            g.symbols.class_methods.get(&type_id)?.get(name).copied()
        }
        _ => None,
    }
}

/// Search every class/struct's method map for a method named `name`.
/// Returns the NodeId iff exactly one class has it (avoids fabricating
/// edges when the same method name lives on multiple types). ENUM owners are
/// skipped: `class_methods` indexes them since LA.30a, and a route handler
/// binding must not turn ambiguous (or change target) because an enum happens
/// to own a same-named method — this pool stays CLASS / STRUCT only.
/// INTERFACE methods are never in the pool: they live in the separate
/// `interface_methods` table (A6.6) precisely so this scan cannot see them.
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

/// A top-level type named `name` across the repo, iff unique:
/// [`unique_global_function`] restricted to STRUCT / CLASS / INTERFACE /
/// ENUM, so a same-named FUNCTION never answers a type lookup (quokka's
/// FUNCTION `repository_provider::UserRepository` beside the STRUCT
/// `user_repository::UserRepository`). Two distinct types -> `None`. The Go
/// call hook's last-resort type lookup (CA.2b); `resolve_type_name` (the
/// generic A6.2a chain) keeps `unique_global_function`, so no other
/// language's CALLS move.
pub(crate) fn unique_global_type(g: &RepoGraph, name: &str) -> Option<NodeId> {
    let is_type = |id: &NodeId| {
        matches!(
            g.nav.kind_by_id.get(id).copied(),
            Some(k) if k == node_kind::STRUCT
                || k == node_kind::CLASS
                || k == node_kind::INTERFACE
                || k == node_kind::ENUM
        )
    };
    let mut hit: Option<NodeId> = None;
    for syms in g.symbols.module_symbols.values() {
        let Some(&id) = syms.get(name).filter(|id| is_type(id)) else {
            continue;
        };
        match hit {
            // One type registered under its MODULE and its PACKAGE.
            Some(existing) if existing == id => {}
            Some(_) => return None,
            None => hit = Some(id),
        }
    }
    hit
}

/// Walk `parent_of` until we hit a module node. For a top-level function this
/// returns its module directly; for a method it walks method → class → module.
/// `pub(crate)` for the Go package hook (`build::GoPackages`, LA.13b), which
/// scopes a call to its caller's file the same way.
pub(crate) fn enclosing_module(nav: &CodeNav, mut id: NodeId) -> Option<NodeId> {
    loop {
        if nav.kind_by_id.get(&id) == Some(&node_kind::MODULE) {
            return Some(id);
        }
        id = *nav.parent_of.get(&id)?;
    }
}

/// The file whose `use` / `using` bindings and top-level symbols a call from
/// `id` resolves through (CB.15): [`enclosing_module`]'s walk, stopping at
/// the first visited node with a `SymbolTable::home_module` entry. A member
/// of a PACKAGE several files open (C# `namespace`, braced PHP `namespace
/// { }`) has one, naming its own file; the PACKAGE node itself hangs under
/// the first file only. Without an entry on the way (every language whose
/// PACKAGEs are per file) this is exactly [`enclosing_module`]. Bounded by
/// the nav's size, so a malformed parent cycle ends.
pub(crate) fn enclosing_home_module(g: &RepoGraph, mut id: NodeId) -> Option<NodeId> {
    let nav = &g.nav;
    for _ in 0..=nav.parent_of.len() {
        if let Some(module) = g.symbols.home_module.get(&id) {
            return Some(*module);
        }
        if nav.kind_by_id.get(&id) == Some(&node_kind::MODULE) {
            return Some(id);
        }
        id = *nav.parent_of.get(&id)?;
    }
    None
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
/// `pub(crate)` for the Go call hook (CA.2b), whose receiver chain `self.a.b`
/// starts at the caller's struct.
pub(crate) fn enclosing_class_or_struct(nav: &CodeNav, start: NodeId) -> Option<NodeId> {
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
/// Needs an exact declared type and an exact method name on it. An
/// INTERFACE-typed field (the canonical DI shape, `IUserService _svc`) binds
/// the interface's own METHOD through `interface_methods` (A6.6); the
/// implementation is one method-level IMPLEMENTS hop further
/// (`emit_method_level_implements`), never guessed here.
///
/// A bare `x.m()` (the `Attribute` arm) names the field only where the
/// language lets a field be read unqualified. In TypeScript / JavaScript and
/// Python it is a parameter, local or global unless it runs inside the
/// constructor, whose same-named parameter has the field's declared type
/// (`constructor(private api: Api)`, `def __init__(self, api: Api)`); the
/// `this.x` / `self.x` form arrives as `ComplexReceiver` and is unaffected.
/// `gate` resolves the caller's language and tallies what it skips.
///
/// LA.35a: the receiver's type comes from [`receiver_type`], which reads the
/// caller's own locals (a parameter, a `let`) before the fields. A type found
/// through a local is not gated: the gate exists because a bare name may be a
/// local rather than the field, and a recorded local answers that.
fn resolve_via_receiver_type(
    g: &RepoGraph,
    site: &CallSite,
    from_module: NodeId,
    gate: &mut BareFieldGate,
) -> Option<NodeId> {
    let method = match &site.qualifier {
        CallQualifier::Attribute { name, .. } | CallQualifier::ComplexReceiver { name, .. } => {
            name.as_str()
        }
        _ => return None,
    };
    if method.is_empty() {
        return None;
    }
    let type_name = receiver_type(g, site)?;
    let type_id = resolve_type_name(g, from_module, type_name)?;
    let hit = g
        .symbols
        .class_methods
        .get(&type_id)
        .and_then(|m| m.get(method).copied())
        .or_else(|| g.symbols.interface_methods.get(&type_id).and_then(|m| m.get(method).copied()))?;
    // A6.2a's gate: an Attribute base the caller does not bind as a local
    // was read as a field of the enclosing type.
    if let CallQualifier::Attribute { base, .. } = &site.qualifier
        && local_type(g, site.from, base).is_none()
        && !gate.admits(g, site.from)
    {
        return None;
    }
    Some(hit)
}

/// The simple type name of a call's receiver, innermost scope first (LA.35a),
/// for A6.2a's receiver pass and the Rust hook (LA.35b).
///
/// The receiver is an `Attribute` base (read as written, as in A6.2a) or a
/// `ComplexReceiver` receiver (`this.` / `self.` stripped by
/// [`receiver_field`], which also rejects chains). A `this.` / `self.`
/// receiver names a field of the enclosing type. Any other receiver is first
/// looked up in the caller's `local_types`: a recorded local decides, a local
/// of unknown type (`""`) returns `None` rather than fall through to a
/// same-named field (it shadows it), and a local recorded as `self.<f>`
/// aliases field `f`. A receiver no local claims falls through to A6.2a's
/// field lookup, so parsers that record no locals see A6.2a exactly.
pub(crate) fn receiver_type<'g>(g: &'g RepoGraph, site: &CallSite) -> Option<&'g str> {
    let (name, is_self) = match &site.qualifier {
        CallQualifier::Attribute { base, .. } => (base.as_str(), false),
        CallQualifier::ComplexReceiver { receiver, .. } => {
            let is_self = receiver.starts_with("this.") || receiver.starts_with("self.");
            (receiver_field(receiver)?, is_self)
        }
        _ => return None,
    };
    if !is_self && let Some(ty) = local_type(g, site.from, name) {
        if ty.is_empty() {
            return None;
        }
        return match ty.strip_prefix("self.") {
            Some(field) => field_type(g, site.from, field),
            None => Some(ty),
        };
    }
    field_type(g, site.from, name)
}

/// The type `scope` records for its local `name` (`""` = unknown type), or
/// `None` when `name` is not a local of `scope`.
fn local_type<'g>(g: &'g RepoGraph, scope: NodeId, name: &str) -> Option<&'g str> {
    g.nav
        .local_types
        .get(&scope)?
        .get(name)
        .map(String::as_str)
}

/// The declared type of `field` on the innermost CLASS / STRUCT / ENUM
/// enclosing `from` (A6.2a's `field_types`).
fn field_type<'g>(g: &'g RepoGraph, from: NodeId, field: &str) -> Option<&'g str> {
    let owner = enclosing_class_or_struct(&g.nav, from)?;
    g.nav
        .field_types
        .get(&owner)?
        .get(field)
        .map(String::as_str)
        .filter(|t| !t.is_empty())
}

/// Where a bare identifier can name an instance field (A6.6, the A6.2b
/// handoff). Keyed by the caller's source-file extension, read off its
/// POSITION cell: the generic pass builds several languages through one
/// builder (`build_typescript` is also the fallback for Kotlin, Swift, Dart),
/// so the builder cannot say which language a call site is in.
#[derive(Default)]
struct BareFieldGate {
    /// NodeId -> index into `g.nodes`, built on the first bare-field hit only.
    /// `resolve_calls` pushes edges, never nodes, so the indices stay valid.
    index: Option<HashMap<NodeId, usize>>,
    /// Bare-field binds skipped outside a constructor, by file extension.
    skipped: HashMap<String, usize>,
}

impl BareFieldGate {
    /// True when a bare `x.m()` in `caller` may bind through `x`'s field type.
    fn admits(&mut self, g: &RepoGraph, caller: NodeId) -> bool {
        let index = self
            .index
            .get_or_insert_with(|| g.nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect());
        let Some(ext) = index
            .get(&caller)
            .and_then(|&i| position_file(&g.nodes[i]))
            .and_then(|file| {
                let base = file.rsplit('/').next().unwrap_or(&file);
                base.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase())
            })
        else {
            // No POSITION: keep A6.2a's behaviour rather than guess a language.
            return true;
        };
        let ctor = match ext.as_str() {
            "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" | "vue" => "constructor",
            "py" | "pyi" => "__init__",
            _ => return true,
        };
        if g.nav.name_by_id.get(&caller).map(String::as_str) == Some(ctor) {
            return true;
        }
        *self.skipped.entry(ext).or_default() += 1;
        false
    }

    /// `[recv] bare field receivers skipped outside constructor: N (ext=a:1,b:2)`,
    /// extensions sorted so the line is stable across runs.
    fn marker(&self) -> Option<String> {
        let total: usize = self.skipped.values().sum();
        if total == 0 {
            return None;
        }
        let mut by_ext: Vec<_> = self.skipped.iter().collect();
        by_ext.sort();
        let exts: Vec<String> = by_ext.iter().map(|(e, n)| format!("{e}:{n}")).collect();
        Some(format!(
            "[recv] bare field receivers skipped outside constructor: {total} (ext={})",
            exts.join(",")
        ))
    }
}

/// A6.6: pair each class-level `impl -> interface` IMPLEMENTS edge's
/// same-named methods into a method-level `impl_method -> iface_method`
/// IMPLEMENTS edge (same direction as the class-level edge), so a trace that
/// lands on an interface method has a hop to every implementation. Runs after
/// `resolve_refs`, which binds the class-level heritage refs. Pairing is by
/// name only (the symbol table has no signatures, as for `class_methods`), so
/// overloads that differ in arity still pair.
///
/// The pairs are sorted and deduped before any edge is pushed: both method
/// tables are HashMaps with a per-process seed, and edge order feeds the
/// store's shard content hashes (engine `byte_identical`). An edge already
/// present is not pushed twice.
///
/// CA.3b: a pair carries the confidence of the class-level edge it rides on,
/// the strongest when several produce one pair: an explicit heritage clause
/// (Strong) keeps its methods Strong, a Go implicit satisfaction (Medium,
/// inferred from the method set) makes its method pairs Medium too.
pub(crate) fn emit_method_level_implements(g: &mut RepoGraph) {
    let mut existing: std::collections::HashSet<(NodeId, NodeId)> = std::collections::HashSet::new();
    let mut pairs: Vec<(NodeId, NodeId, Confidence)> = Vec::new();
    for e in &g.edges {
        if e.category != edge_category::IMPLEMENTS {
            continue;
        }
        existing.insert((e.from, e.to));
        if g.nav.kind_by_id.get(&e.to) != Some(&node_kind::INTERFACE) {
            continue;
        }
        let (Some(impl_ms), Some(iface_ms)) =
            (g.symbols.class_methods.get(&e.from), g.symbols.interface_methods.get(&e.to))
        else {
            continue;
        };
        for (name, &iface_mid) in iface_ms {
            if let Some(&impl_mid) = impl_ms.get(name) {
                pairs.push((impl_mid, iface_mid, e.confidence));
            }
        }
    }
    pairs.sort_unstable_by_key(|(a, b, c)| (a.0, b.0, confidence_rank(*c)));
    pairs.dedup_by_key(|(a, b, _)| (*a, *b));
    pairs.retain(|(a, b, _)| !existing.contains(&(*a, *b)));
    for &(from, to, confidence) in &pairs {
        let ev = graph_evidence("graph:iface", "same_name");
        push_edge_with(g, from, to, edge_category::IMPLEMENTS, ev, confidence);
    }
    if !pairs.is_empty() {
        eprintln!(
            "[iface] method-level implements: {} (interfaces={})",
            pairs.len(),
            g.symbols.interface_methods.len()
        );
    }
}

/// Push one resolved edge, carrying `ev` as its EVIDENCE cell: the graph
/// mechanism and branch that bound it ([`graph_evidence`], LC.3d). The
/// engine's `graph:build` stage stamp then leaves it alone.
pub(crate) fn push_edge(
    g: &mut RepoGraph,
    from: NodeId,
    to: NodeId,
    category: EdgeCategoryId,
    ev: Evidence,
) {
    push_edge_with(g, from, to, category, ev, Confidence::Strong);
}

/// [`push_edge`] at `confidence`: an edge the graph crate infers rather than
/// reads (CA.3b, a Go implicit method pair) is pushed below Strong.
pub(crate) fn push_edge_with(
    g: &mut RepoGraph,
    from: NodeId,
    to: NodeId,
    category: EdgeCategoryId,
    ev: Evidence,
    confidence: Confidence,
) {
    let mut e = Edge::new(from, to, category, confidence);
    evidence::attach(&mut e, ev);
    g.edges.push(e);
}

/// Strong < Medium < Weak, for keeping the strongest of several edges that
/// produce one pair ([`emit_method_level_implements`]).
fn confidence_rank(c: Confidence) -> u8 {
    match c {
        Confidence::Strong => 0,
        Confidence::Medium => 1,
        Confidence::Weak => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{build_dotted, build_python, build_typescript};
    use crate::test_support::repo;
    use glia_code_domain::{FileParse, GRAPH_TYPE, ImportStmt, ImportTarget};
    use glia_core::Node;
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
                line: 0,
            }],
            calls: vec![CallSite {
                from: caller,
                qualifier: CallQualifier::Attribute {
                    base: "Accounts".to_string(),
                    name: "get_user".to_string(),
                },
                line: 0,
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
            kind: glia_core::NodeKindId,
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
            line: 0,
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
        let site = CallSite { from: a, qualifier: CallQualifier::SelfMethod("b".to_string()), line: 0 };
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
        let site = CallSite { from: inner, qualifier: CallQualifier::SelfMethod("b".to_string()), line: 0 };
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
            vec![CallSite { from: f, qualifier: attr("E", "pick"), line: 0 }],
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
            line: 0,
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
            line: 0,
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
            vec![CallSite { from: f, qualifier: attr("E", "RED"), line: 0 }],
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
            line: 0,
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
        (s.file(imports, vec![CallSite { from: get, qualifier, line: 0 }], vec![]), a, get)
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
        caller.calls.push(CallSite { from: run, qualifier: attr("repo", "find"), line: 0 });
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

    // ---- LA.35a: locals before fields ----------------------------------------

    fn recv(receiver: &str, name: &str) -> CallQualifier {
        CallQualifier::ComplexReceiver { receiver: receiver.to_string(), name: name.to_string() }
    }

    /// `m3`: `struct Index { find() }` — a second type with a `find`.
    fn index_module() -> (FileParse, NodeId) {
        let mut s = Shape::new();
        let m3 = s.add(node_kind::MODULE, "m3", None);
        let index = s.add(node_kind::STRUCT, "m3::Index", Some(m3));
        let find = s.add(node_kind::METHOD, "m3::Index::find", Some(index));
        (s.file(vec![], vec![], vec![]), find)
    }

    /// Rust `fn free(r: &UserRepo) { r.find() }`: a free fn has no enclosing
    /// type, so only its local `r` can type the receiver.
    #[test]
    fn local_type_binds_complex_receiver() {
        let (repo_file, _, find) = repo_module();
        let mut s = Shape::new();
        let m1 = s.add(node_kind::MODULE, "m1", None);
        let free = s.add(node_kind::FUNCTION, "m1::free", Some(m1));
        s.nav.record_local_type(free, "r", "UserRepo");
        let calls = vec![
            CallSite { from: free, qualifier: recv("r", "find"), line: 0 },
            // Not a local of `free`: nothing to type it by.
            CallSite { from: free, qualifier: recv("q", "find"), line: 0 },
        ];
        let g = build_dotted(repo(), vec![repo_file, s.file(vec![], calls, vec![])]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(free, find)]);
        assert_eq!(g.unresolved_calls.len(), 1);
    }

    /// `let repo = index(); repo.find()` inside a type with a field
    /// `repo: UserRepo`: the local of unknown type shadows the field, in both
    /// qualifier shapes, and a `self.repo` receiver still reads the field.
    #[test]
    fn unknown_local_shadows_field() {
        let (repo_file, _, find) = repo_module();
        let (mut caller, _, get) = caller_module(recv("repo", "find"), vec![]);
        caller.nav.record_local_type(get, "repo", "");
        caller.calls.push(CallSite { from: get, qualifier: attr("repo", "find"), line: 0 });
        let g = build_dotted(repo(), vec![repo_file.clone(), caller.clone()]).unwrap();
        assert!(edges_of(&g, edge_category::CALLS).is_empty());
        assert_eq!(g.unresolved_calls.len(), 2);

        caller.calls = vec![CallSite { from: get, qualifier: recv("self.repo", "find"), line: 0 }];
        let g = build_dotted(repo(), vec![repo_file, caller]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(get, find)]);
    }

    /// Field `repo: UserRepo`, local `repo: Index`: the local is the inner
    /// scope, so `repo.find()` binds `Index::find`.
    #[test]
    fn local_takes_precedence_over_field() {
        let (repo_file, _, user_find) = repo_module();
        let (index_file, index_find) = index_module();
        let (mut caller, _, get) = caller_module(recv("repo", "find"), vec![]);
        caller.nav.record_local_type(get, "repo", "Index");
        let g = build_dotted(repo(), vec![repo_file, index_file, caller]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(get, index_find)]);
        assert!(!edges_of(&g, edge_category::CALLS).contains(&(get, user_find)));
    }

    /// `let r = &self.repo; r.find()`: the local aliases the field, whose
    /// declared type binds the call; an alias to a field the type does not
    /// declare binds nothing.
    #[test]
    fn self_field_alias_resolves_through_field_types() {
        let (repo_file, _, find) = repo_module();
        let (mut caller, _, get) = caller_module(recv("r", "find"), vec![]);
        caller.nav.record_local_type(get, "r", "self.repo");
        caller.nav.record_local_type(get, "o", "self.other");
        caller.calls.push(CallSite { from: get, qualifier: recv("o", "find"), line: 0 });
        let g = build_dotted(repo(), vec![repo_file, caller]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(get, find)]);
        assert_eq!(g.unresolved_calls.len(), 1);
    }

    /// `m2`: `interface UserRepo { find() }` — an INTERFACE-typed field's
    /// declared type (A6.6).
    fn iface_module() -> (FileParse, NodeId, NodeId) {
        let mut s = Shape::new();
        let m2 = s.add(node_kind::MODULE, "m2", None);
        let iface = s.add(node_kind::INTERFACE, "m2::UserRepo", Some(m2));
        let find = s.add(node_kind::METHOD, "m2::UserRepo::find", Some(iface));
        (s.file(vec![], vec![], vec![]), iface, find)
    }

    /// A6.6: an INTERFACE-typed field (`IUserRepo _repo; _repo.find()`)
    /// binds the interface's own METHOD through `interface_methods`. Before
    /// A6.6 the INTERFACE owned no method table and the call stayed unresolved.
    #[test]
    fn interface_typed_field_binds_the_interface_method() {
        let (iface_file, _, find) = iface_module();
        let (caller, _, get) = caller_module(attr("repo", "find"), vec![]);
        let g = build_dotted(repo(), vec![iface_file, caller]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(get, find)]);
        assert!(g.unresolved_calls.is_empty());
    }

    /// An unknown method on an interface-typed field never binds.
    #[test]
    fn interface_typed_field_needs_an_exact_method() {
        let (iface_file, _, _) = iface_module();
        let (caller, _, _) = caller_module(attr("repo", "missing"), vec![]);
        let g = build_dotted(repo(), vec![iface_file, caller]).unwrap();
        assert!(edges_of(&g, edge_category::CALLS).is_empty());
        assert_eq!(g.unresolved_calls.len(), 1);
    }

    /// `import m2.UserRepo` + `UserRepo.find()`: a statically-qualified call on
    /// an imported INTERFACE binds through `resolve_attribute_target`'s
    /// INTERFACE arm.
    #[test]
    fn attribute_call_binds_interface_base() {
        let (iface_file, _, find) = iface_module();
        let mut s = Shape::new();
        let m1 = s.add(node_kind::MODULE, "m1", None);
        let f = s.add(node_kind::FUNCTION, "m1::f", Some(m1));
        let caller = s.file(
            vec![import_symbol("m1", "m2", "UserRepo")],
            vec![CallSite { from: f, qualifier: attr("UserRepo", "find"), line: 0 }],
            vec![],
        );
        let g = build_dotted(repo(), vec![iface_file, caller]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(f, find)]);
    }

    // ---- A6.6: the bare-field gate (handoff from A6.2b) ----------------------

    /// Give the node `id` a POSITION cell in `file`.
    fn place(file: &mut FileParse, id: NodeId, path: &str) {
        let node = file.nodes.iter_mut().find(|n| n.id == id).expect("node in file");
        node.cells.push(glia_core::Cell {
            kind: cell_type::POSITION,
            payload: glia_core::CellPayload::Json(format!(
                "{{\"file\":\"{path}\",\"start_line\":1,\"end_line\":2}}"
            )),
        });
    }

    /// `m1`: `class A { repo: UserRepo; <method>() { <qualifier> } }` where the
    /// caller lives in `path` and is named `method`.
    fn placed_caller(
        qualifier: CallQualifier,
        method: &str,
        path: &str,
    ) -> (FileParse, NodeId) {
        let mut s = Shape::new();
        let m1 = s.add(node_kind::MODULE, "m1", None);
        let a = s.add(node_kind::CLASS, "m1::A", Some(m1));
        let caller = s.add(node_kind::METHOD, &format!("m1::A::{method}"), Some(a));
        s.nav.record_field_type(a, "repo", "UserRepo");
        let mut file = s.file(vec![], vec![CallSite { from: caller, qualifier, line: 0 }], vec![]);
        place(&mut file, caller, path);
        (file, caller)
    }

    /// TypeScript: `shadow(repo: Other) { repo.find() }` names the PARAMETER —
    /// a field is only reachable as `this.repo` — so the bare form must not
    /// bind through the field's type (the A6.2b-measured false positive).
    #[test]
    fn bare_receiver_outside_constructor_does_not_bind_in_typescript() {
        let (repo_file, _, _) = repo_module();
        let (caller, _) = placed_caller(attr("repo", "find"), "shadow", "src/a.service.ts");
        let g = build_typescript(repo(), vec![repo_file, caller], |_, _| None).unwrap();
        assert!(edges_of(&g, edge_category::CALLS).is_empty());
        assert_eq!(g.unresolved_calls.len(), 1);
    }

    /// Inside the constructor the bare name is the parameter property, whose
    /// type IS the field's: `constructor(private repo: UserRepo) { repo.find() }`.
    #[test]
    fn bare_receiver_inside_constructor_binds_in_typescript() {
        let (repo_file, _, find) = repo_module();
        let (caller, ctor) =
            placed_caller(attr("repo", "find"), "constructor", "src/a.service.ts");
        let g = build_typescript(repo(), vec![repo_file, caller], |_, _| None).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(ctor, find)]);
    }

    /// `this.repo.find()` is a ComplexReceiver: never gated, in any method.
    #[test]
    fn this_qualified_receiver_is_not_gated_in_typescript() {
        let (repo_file, _, find) = repo_module();
        let qualifier = CallQualifier::ComplexReceiver {
            receiver: "this.repo".to_string(),
            name: "find".to_string(),
        };
        let (caller, get) = placed_caller(qualifier, "get", "src/a.service.ts");
        let g = build_typescript(repo(), vec![repo_file, caller], |_, _| None).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(get, find)]);
    }

    /// Python: bare `repo.find()` outside `__init__` does not bind; inside
    /// `__init__` it is the annotated parameter and does.
    #[test]
    fn bare_receiver_is_gated_to_init_in_python() {
        let (repo_file, _, find) = repo_module();
        let (outside, _) = placed_caller(attr("repo", "find"), "run", "svc/a.py");
        let g = build_python(repo(), vec![repo_file, outside]).unwrap();
        assert!(edges_of(&g, edge_category::CALLS).is_empty());

        let (repo_file, _, _) = repo_module();
        let (inside, init) = placed_caller(attr("repo", "find"), "__init__", "svc/a.py");
        let g = build_python(repo(), vec![repo_file, inside]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(init, find)]);
    }

    /// C#: a field is readable unqualified, so `_repo.Find()` in any method
    /// keeps binding (A6.2a's csharp-field-dispatch shape).
    #[test]
    fn bare_receiver_binds_in_any_method_in_csharp() {
        let (repo_file, _, find) = repo_module();
        let (caller, get) = placed_caller(attr("repo", "find"), "Get", "Services/A.cs");
        let g = build_dotted(repo(), vec![repo_file, caller]).unwrap();
        assert_eq!(edges_of(&g, edge_category::CALLS), vec![(get, find)]);
    }

    #[test]
    fn bare_field_gate_marker_counts_by_extension() {
        let mut gate = BareFieldGate::default();
        assert_eq!(gate.marker(), None);
        gate.skipped.insert("ts".to_string(), 2);
        gate.skipped.insert("py".to_string(), 1);
        assert_eq!(
            gate.marker().as_deref(),
            Some("[recv] bare field receivers skipped outside constructor: 3 (ext=py:1,ts:2)")
        );
    }

    // ---- A6.3: TypeScript heritage through UnresolvedRef ---------------------

    /// `class <from> extends|implements <name>` as the parser emits it since A6.3.
    fn heritage_ref(
        from: NodeId,
        module: NodeId,
        name: &str,
        cat: EdgeCategoryId,
    ) -> UnresolvedRef {
        UnresolvedRef {
            from,
            from_module: module,
            qualifier: CallQualifier::Bare(name.to_string()),
            category: cat,
            line: 0,
        }
    }

    fn heritage_edges(g: &RepoGraph) -> Vec<(NodeId, NodeId, EdgeCategoryId, Confidence)> {
        g.edges
            .iter()
            .filter(|e| {
                e.category == edge_category::INHERITS_FROM
                    || e.category == edge_category::IMPLEMENTS
            })
            .map(|e| (e.from, e.to, e.category, e.confidence))
            .collect()
    }

    /// The one regression A6.3 can cause: `class X extends Base implements
    /// IFoo` with both supertypes in the SAME file used to bind through the
    /// parser's name-derived id. It must still bind, now through the module's
    /// own symbols. A second `Base` / `IFoo` in another file makes the
    /// repo-wide unique-name fallback ambiguous, so only the same-module step
    /// can produce these edges.
    #[test]
    fn same_file_heritage_still_binds() {
        let mut s = Shape::new();
        let m1 = s.add(node_kind::MODULE, "src::a", None);
        let x = s.add(node_kind::CLASS, "src::a::X", Some(m1));
        let base = s.add(node_kind::CLASS, "src::a::Base", Some(m1));
        let ifoo = s.add(node_kind::INTERFACE, "src::a::IFoo", Some(m1));
        let file = s.file(
            vec![],
            vec![],
            vec![
                heritage_ref(x, m1, "Base", edge_category::INHERITS_FROM),
                heritage_ref(x, m1, "IFoo", edge_category::IMPLEMENTS),
            ],
        );
        let mut other = Shape::new();
        let m2 = other.add(node_kind::MODULE, "src::b", None);
        other.add(node_kind::CLASS, "src::b::Base", Some(m2));
        other.add(node_kind::INTERFACE, "src::b::IFoo", Some(m2));
        let g =
            build_typescript(repo(), vec![file, other.file(vec![], vec![], vec![])], |_, _| None)
                .unwrap();
        assert_eq!(
            heritage_edges(&g),
            vec![
                (x, base, edge_category::INHERITS_FROM, Confidence::Strong),
                (x, ifoo, edge_category::IMPLEMENTS, Confidence::Strong),
            ]
        );
        assert!(g.unresolved_refs.is_empty());
    }

    /// Cross-file: `import { Base } from "./base"` binds the heritage name to
    /// the imported class even when another `Base` exists elsewhere; an
    /// external base (`extends Component` from a package) stays an unresolved
    /// ref instead of an edge into a fabricated id.
    #[test]
    fn imported_heritage_binds_and_external_base_stays_unresolved() {
        let mut b = Shape::new();
        let mb = b.add(node_kind::MODULE, "src::base", None);
        let base = b.add(node_kind::CLASS, "src::base::Base", Some(mb));
        let mut dup = Shape::new();
        let md = dup.add(node_kind::MODULE, "src::legacy", None);
        dup.add(node_kind::CLASS, "src::legacy::Base", Some(md));
        let mut c = Shape::new();
        let mc = c.add(node_kind::MODULE, "src::child", None);
        let child = c.add(node_kind::CLASS, "src::child::Child", Some(mc));
        let widget = c.add(node_kind::CLASS, "src::child::Widget", Some(mc));
        let child_file = c.file(
            vec![import_symbol("src::child", "./base", "Base")],
            vec![],
            vec![
                heritage_ref(child, mc, "Base", edge_category::INHERITS_FROM),
                heritage_ref(widget, mc, "Component", edge_category::INHERITS_FROM),
            ],
        );
        let g = build_typescript(
            repo(),
            vec![b.file(vec![], vec![], vec![]), dup.file(vec![], vec![], vec![]), child_file],
            |_, spec| (spec == "./base").then(|| "src::base".to_string()),
        )
        .unwrap();
        assert_eq!(
            heritage_edges(&g),
            vec![(child, base, edge_category::INHERITS_FROM, Confidence::Strong)]
        );
        assert_eq!(g.unresolved_refs.len(), 1);
        assert_eq!(g.unresolved_refs[0].qualifier, CallQualifier::Bare("Component".to_string()));
    }

    /// A TS default import binds its local name to the MODULE. Heritage must
    /// not become CLASS -> MODULE: it looks through to the module's same-named
    /// class, and a default import whose local name matches nothing there
    /// stays unresolved even when a same-named class exists elsewhere (the
    /// name is import-bound, so the repo-wide fallback would guess).
    #[test]
    fn default_imported_heritage_looks_through_the_module() {
        let mut b = Shape::new();
        let mb = b.add(node_kind::MODULE, "src::base", None);
        let base = b.add(node_kind::CLASS, "src::base::Base", Some(mb));
        let mut o = Shape::new();
        let mo = o.add(node_kind::MODULE, "src::other", None);
        o.add(node_kind::CLASS, "src::other::Renamed", Some(mo));
        let mut c = Shape::new();
        let mc = c.add(node_kind::MODULE, "src::child", None);
        let child = c.add(node_kind::CLASS, "src::child::Child", Some(mc));
        let alias = c.add(node_kind::CLASS, "src::child::Aliased", Some(mc));
        let default_import = |local: &str| ImportStmt {
            from_module: "src::child".to_string(),
            target: ImportTarget::Symbol {
                module: "./base".to_string(),
                name: "default".to_string(),
                alias: Some(local.to_string()),
                level: 0,
            },
            line: 0,
        };
        let child_file = c.file(
            vec![default_import("Base"), default_import("Renamed")],
            vec![],
            vec![
                heritage_ref(child, mc, "Base", edge_category::INHERITS_FROM),
                heritage_ref(alias, mc, "Renamed", edge_category::INHERITS_FROM),
            ],
        );
        let g = build_typescript(
            repo(),
            vec![b.file(vec![], vec![], vec![]), o.file(vec![], vec![], vec![]), child_file],
            |_, spec| (spec == "./base").then(|| "src::base".to_string()),
        )
        .unwrap();
        assert_eq!(
            heritage_edges(&g),
            vec![(child, base, edge_category::INHERITS_FROM, Confidence::Strong)]
        );
        assert_eq!(g.unresolved_refs.len(), 1);
        assert_eq!(g.unresolved_refs[0].from, alias);
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

    // ---- LD.7a: interface -> super-interface heritage ------------------------

    /// `package shop` over three Java files: `Readable.java` declares the
    /// super-interface, `Catalog.java` extends it — the parser's INHERITS_FROM
    /// ref, bound across files by the global unique-type fallback.
    fn iface_extends_files(readable_path: &str) -> (Vec<FileParse>, NodeId, NodeId) {
        let mut r = Shape::new();
        let rm = r.add(node_kind::MODULE, "shop::Readable", None);
        let readable = r.add(node_kind::INTERFACE, "shop::Readable", Some(rm));
        let mut readable_file = r.file(vec![], vec![], vec![]);
        place(&mut readable_file, readable, readable_path);
        let mut c = Shape::new();
        let cm = c.add(node_kind::MODULE, "shop::Catalog", None);
        let catalog = c.add(node_kind::INTERFACE, "shop::Catalog", Some(cm));
        let mut catalog_file = c.file(
            vec![],
            vec![],
            vec![heritage_ref(catalog, cm, "Readable", edge_category::INHERITS_FROM)],
        );
        place(&mut catalog_file, catalog, "src/shop/Catalog.java");
        (vec![readable_file, catalog_file], catalog, readable)
    }

    #[test]
    fn interface_extends_binds_across_files() {
        let (files, catalog, readable) = iface_extends_files("src/shop/Readable.java");
        let g = build_dotted(repo(), files).unwrap();
        assert_eq!(
            heritage_edges(&g),
            vec![(catalog, readable, edge_category::INHERITS_FROM, Confidence::Strong)]
        );
        assert!(g.unresolved_refs.is_empty());
        let tally = IfaceExtends { bound: 1, unresolved: 0, ifaces: vec![catalog] };
        assert_eq!(
            tally.marker(&g).as_deref(),
            Some("[heritage] interface-extends bound=1 unresolved=0 ext=java")
        );
    }

    /// Two packages each declaring a `Readable`: the bare name is ambiguous for
    /// the global fallback, so the ref stays unresolved (counted), never
    /// first-wins.
    #[test]
    fn ambiguous_super_interface_stays_unresolved() {
        let (mut files, _, _) = iface_extends_files("src/shop/Readable.java");
        let mut o = Shape::new();
        let om = o.add(node_kind::MODULE, "other::Readable", None);
        o.add(node_kind::INTERFACE, "other::Readable", Some(om));
        files.push(o.file(vec![], vec![], vec![]));
        let g = build_dotted(repo(), files).unwrap();
        assert!(heritage_edges(&g).is_empty());
        assert_eq!(g.unresolved_refs.len(), 1);
    }

    /// Only INHERITS_FROM out of an INTERFACE counts toward the marker: a
    /// CLASS's `extends` and an IMPLEMENTS ref do not, and the marker is silent
    /// when nothing counted.
    #[test]
    fn interface_extends_counts_only_interface_inherits_from() {
        let (files, catalog, _) = iface_extends_files("src/shop/Readable.java");
        let mut k = Shape::new();
        let km = k.add(node_kind::MODULE, "shop::PgCatalog", None);
        let class = k.add(node_kind::CLASS, "shop::PgCatalog", Some(km));
        let mut all = files;
        all.push(k.file(vec![], vec![], vec![]));
        let g = build_dotted(repo(), all).unwrap();
        let m = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "shop::Catalog");
        assert!(extends_interface(&g, &heritage_ref(catalog, m, "Readable", edge_category::INHERITS_FROM)));
        assert!(!extends_interface(&g, &heritage_ref(catalog, m, "Readable", edge_category::IMPLEMENTS)));
        assert!(!extends_interface(&g, &heritage_ref(class, km, "Base", edge_category::INHERITS_FROM)));
        assert_eq!(IfaceExtends::default().marker(&g), None);
    }

    // ---- LC.3d: the resolving branch in each edge's evidence -----------------

    /// `(emitter, rule)` of the one `from -> to` edge of `category`.
    fn evidence_rule(
        g: &RepoGraph,
        from: NodeId,
        to: NodeId,
        category: EdgeCategoryId,
    ) -> Option<(String, Option<String>)> {
        g.edges
            .iter()
            .find(|e| e.from == from && e.to == to && e.category == category)
            .and_then(glia_code_domain::evidence::Evidence::of)
            .map(|ev| (ev.emitter, ev.rule))
    }

    fn calls_rule(rule: &str) -> Option<(String, Option<String>)> {
        Some(("graph:calls".to_string(), Some(rule.to_string())))
    }

    fn bare(name: &str) -> CallQualifier {
        CallQualifier::Bare(name.to_string())
    }

    /// Three graphs, each resolvable by exactly one Bare branch: an import
    /// binding, a same-module def, the enclosing PACKAGE's def.
    #[test]
    fn bare_call_names_its_branch() {
        // import_binding: `from m2 import helper` + `helper()`.
        let mut s2 = Shape::new();
        let m2 = s2.add(node_kind::MODULE, "m2", None);
        let helper = s2.add(node_kind::FUNCTION, "m2::helper", Some(m2));
        let mut s1 = Shape::new();
        let m1 = s1.add(node_kind::MODULE, "m1", None);
        let f = s1.add(node_kind::FUNCTION, "m1::f", Some(m1));
        let caller = s1.file(
            vec![import_symbol("m1", "m2", "helper")],
            vec![CallSite { from: f, qualifier: bare("helper"), line: 0 }],
            vec![],
        );
        let g = build_dotted(repo(), vec![s2.file(vec![], vec![], vec![]), caller]).unwrap();
        assert_eq!(evidence_rule(&g, f, helper, edge_category::CALLS), calls_rule("import_binding"));

        // module_symbol: a sibling top-level def of the caller's module.
        let mut s = Shape::new();
        let m = s.add(node_kind::MODULE, "m", None);
        let a = s.add(node_kind::FUNCTION, "m::a", Some(m));
        let b = s.add(node_kind::FUNCTION, "m::b", Some(m));
        let file = s.file(vec![], vec![CallSite { from: a, qualifier: bare("b"), line: 0 }], vec![]);
        let g = build_dotted(repo(), vec![file]).unwrap();
        assert_eq!(evidence_rule(&g, a, b, edge_category::CALLS), calls_rule("module_symbol"));

        // package_symbol: Elixir `defmodule` PACKAGE siblings, which the file
        // MODULE's own symbols do not list.
        let mut s = Shape::new();
        let m = s.add(node_kind::MODULE, "lib", None);
        let pkg = s.add(node_kind::PACKAGE, "lib::MyApp", Some(m));
        let a = s.add(node_kind::FUNCTION, "lib::MyApp::a", Some(pkg));
        let b = s.add(node_kind::FUNCTION, "lib::MyApp::b", Some(pkg));
        let file = s.file(vec![], vec![CallSite { from: a, qualifier: bare("b"), line: 0 }], vec![]);
        let g = build_dotted(repo(), vec![file]).unwrap();
        assert_eq!(evidence_rule(&g, a, b, edge_category::CALLS), calls_rule("package_symbol"));
    }

    /// A HANDLED_BY ref no import or same-module def can bind falls back to
    /// the repo-unique name, and says so: rule `global_unique`.
    #[test]
    fn global_fallback_is_named() {
        let mut s2 = Shape::new();
        let m2 = s2.add(node_kind::MODULE, "m2", None);
        let handler = s2.add(node_kind::FUNCTION, "m2::list_users", Some(m2));
        let mut s1 = Shape::new();
        let m1 = s1.add(node_kind::MODULE, "m1", None);
        let routes = s1.add(node_kind::FUNCTION, "m1::routes", Some(m1));
        let handled = UnresolvedRef {
            from: routes,
            from_module: m1,
            qualifier: bare("list_users"),
            category: edge_category::HANDLED_BY,
            line: 0,
        };
        let caller = s1.file(vec![], vec![], vec![handled]);
        let g = build_dotted(repo(), vec![s2.file(vec![], vec![], vec![]), caller]).unwrap();
        assert_eq!(
            evidence_rule(&g, routes, handler, edge_category::HANDLED_BY),
            Some(("graph:refs".to_string(), Some("global_unique".to_string())))
        );
    }

    // ---- CA.5a: a type-qualified HANDLED_BY base binds its own method -----

    /// A HANDLED_BY ref `Attribute { base: ty, name }` from `route` in `module`.
    fn type_handler(route: NodeId, module: NodeId, ty: &str, name: &str) -> UnresolvedRef {
        UnresolvedRef {
            from: route,
            from_module: module,
            qualifier: CallQualifier::Attribute { base: ty.to_string(), name: name.to_string() },
            category: edge_category::HANDLED_BY,
            line: 7,
        }
    }

    fn refs_rule(rule: &str) -> Option<(String, Option<String>)> {
        Some(("graph:refs".to_string(), Some(rule.to_string())))
    }

    /// Kina's shape: two handler types in two files of one package, each with
    /// `List`, each registering `h.List` in its own `RegisterRoutes`. The
    /// repo-unique method fallback binds neither (two `List`s); the type the
    /// registering module declares binds each route to its own type's method.
    #[test]
    fn type_qualified_handler_binds_its_own_method() {
        let mut t = Shape::new();
        let tm = t.add(node_kind::MODULE, "handlers::tokens", None);
        let tokens = t.add(node_kind::STRUCT, "handlers::tokens::TokensHandler", Some(tm));
        let tokens_list =
            t.add(node_kind::METHOD, "handlers::tokens::TokensHandler::List", Some(tokens));
        let get_tokens = t.add(node_kind::ROUTE, "GET /tokens", None);
        let tokens_file =
            t.file(vec![], vec![], vec![type_handler(get_tokens, tm, "TokensHandler", "List")]);

        let mut o = Shape::new();
        let om = o.add(node_kind::MODULE, "handlers::offers", None);
        let offers = o.add(node_kind::STRUCT, "handlers::offers::OffersHandler", Some(om));
        let offers_list =
            o.add(node_kind::METHOD, "handlers::offers::OffersHandler::List", Some(offers));
        let get_offers = o.add(node_kind::ROUTE, "GET /offers", None);
        let offers_file =
            o.file(vec![], vec![], vec![type_handler(get_offers, om, "OffersHandler", "List")]);

        let g = crate::build::build_go(repo(), vec![tokens_file, offers_file]).unwrap();
        let handled: Vec<(NodeId, NodeId)> = g
            .edges
            .iter()
            .filter(|e| e.category == edge_category::HANDLED_BY)
            .map(|e| (e.from, e.to))
            .collect();
        assert_eq!(handled.len(), 2, "{handled:?}");
        assert!(handled.contains(&(get_tokens, tokens_list)));
        assert!(handled.contains(&(get_offers, offers_list)));
        assert_eq!(
            evidence_rule(&g, get_tokens, tokens_list, edge_category::HANDLED_BY),
            refs_rule("type_method")
        );
        assert_eq!(
            evidence_rule(&g, get_offers, offers_list, edge_category::HANDLED_BY),
            refs_rule("type_method")
        );
        assert!(g.unresolved_refs.is_empty());
    }

    /// A type the registering module does not declare (a split receiver: the
    /// struct lives in another file) takes the repo-unique method fallback
    /// exactly as before, and a base naming a FUNCTION there is no type.
    #[test]
    fn type_qualified_handler_falls_back_when_the_type_is_elsewhere() {
        let mut s = Shape::new();
        let sm = s.add(node_kind::MODULE, "handlers::users", None);
        let users = s.add(node_kind::STRUCT, "handlers::users::UsersHandler", Some(sm));
        let show = s.add(node_kind::METHOD, "handlers::users::UsersHandler::Show", Some(users));
        let types_file = s.file(vec![], vec![], vec![]);

        let mut r = Shape::new();
        let rm = r.add(node_kind::MODULE, "handlers::routes", None);
        // A same-module FUNCTION named like the type is not a receiver type.
        r.add(node_kind::FUNCTION, "handlers::routes::Other", Some(rm));
        let get_user = r.add(node_kind::ROUTE, "GET /users/{id}", None);
        let get_other = r.add(node_kind::ROUTE, "GET /other", None);
        let routes_file = r.file(
            vec![],
            vec![],
            vec![
                type_handler(get_user, rm, "UsersHandler", "Show"),
                type_handler(get_other, rm, "Other", "Missing"),
            ],
        );

        let g = crate::build::build_go(repo(), vec![types_file, routes_file]).unwrap();
        assert_eq!(
            evidence_rule(&g, get_user, show, edge_category::HANDLED_BY),
            refs_rule("global_unique_method")
        );
        assert_eq!(g.unresolved_refs.len(), 1);
        assert_eq!(g.unresolved_refs[0].from, get_other);
    }

    /// The `[evidence-graph]` marker: silent until a call resolves, then every
    /// counter in its fixed order.
    #[test]
    fn evidence_marker_is_fixed_order() {
        let mut t = EvidenceTally::default();
        t.refs[Branch::GlobalUnique as usize] = 4;
        assert_eq!(t.marker(), None, "refs alone do not fire it");
        t.calls[Branch::ImportBinding as usize] = 3;
        t.calls[Branch::ReceiverType as usize] = 1;
        t.extra_hook = 2;
        t.refs[Branch::EnumMember as usize] = 5;
        assert_eq!(
            t.marker().as_deref(),
            Some(
                "[evidence-graph] calls import_binding=3 module_symbol=0 package_symbol=0 \
                 attribute=0 self_method=0 receiver_type=1 extra_hook=2 refs global_unique=4 \
                 enum_member=5"
            )
        );
    }

    /// CA.3b: a method-level IMPLEMENTS pair carries its class-level edge's
    /// confidence. A Java `class Repo implements Store` (a heritage ref bound
    /// Strong) keeps its method pair Strong; with a Medium duplicate of the
    /// class edge listed first, the strongest still wins.
    #[test]
    fn java_explicit_implements_pair_stays_strong() {
        let r = repo();
        let id = |kind, q: &str| NodeId::from_parts(GRAPH_TYPE, r, kind, q);
        let (m, iface, iface_get, cls, cls_get) = (
            id(node_kind::MODULE, "app::Repo"),
            id(node_kind::INTERFACE, "app::Repo::Store"),
            id(node_kind::METHOD, "app::Repo::Store::get"),
            id(node_kind::CLASS, "app::Repo::Repo"),
            id(node_kind::METHOD, "app::Repo::Repo::get"),
        );
        let mut nav = CodeNav::default();
        nav.record(m, "Repo", "app::Repo", node_kind::MODULE, None);
        nav.record(iface, "Store", "app::Repo::Store", node_kind::INTERFACE, Some(m));
        nav.record(iface_get, "get", "app::Repo::Store::get", node_kind::METHOD, Some(iface));
        nav.record(cls, "Repo", "app::Repo::Repo", node_kind::CLASS, Some(m));
        nav.record(cls_get, "get", "app::Repo::Repo::get", node_kind::METHOD, Some(cls));
        let node = |id| Node { id, repo: r, confidence: Confidence::Strong, cells: vec![] };
        let file = FileParse {
            nodes: [m, iface, iface_get, cls, cls_get].into_iter().map(node).collect(),
            edges: vec![],
            imports: vec![],
            calls: vec![],
            refs: vec![UnresolvedRef {
                from: cls,
                from_module: m,
                qualifier: CallQualifier::Bare("Store".to_string()),
                category: edge_category::IMPLEMENTS,
                line: 0,
            }],
            nav,
            properties: HashSet::new(),
        };
        let mut g = build_dotted(r, vec![file]).unwrap();
        let conf = |g: &RepoGraph, from, to| {
            g.edges
                .iter()
                .find(|e| e.from == from && e.to == to && e.category == edge_category::IMPLEMENTS)
                .map(|e| e.confidence)
        };
        assert_eq!(conf(&g, cls, iface), Some(Confidence::Strong));
        assert_eq!(conf(&g, cls_get, iface_get), Some(Confidence::Strong));

        g.edges.retain(|e| e.from != cls_get);
        g.edges.insert(0, Edge::new(cls, iface, edge_category::IMPLEMENTS, Confidence::Medium));
        emit_method_level_implements(&mut g);
        assert_eq!(conf(&g, cls_get, iface_get), Some(Confidence::Strong), "strongest class edge");
    }
}
