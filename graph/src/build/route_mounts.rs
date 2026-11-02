//! GD.40: include-style route mounts. A route file mounted under a prefix BY
//! REFERENCE to its module gives each of its routes the full path, through
//! CB.20's mount fixpoint (`crate::go_mounts`) with a module slot. A child
//! of `build.rs`: `build_python` runs [`bind_module_mounts`] once the surface
//! (the symbol table) is built and before the bodies are stitched, so the
//! re-keyed ROUTEs are the ones every later pass sees.
//!
//! Django first. The Python parser mints each `path` / `re_path` / `url`
//! pattern of a urlconf (a module that assigns `urlpatterns`) under the
//! provisional qname `ANY <mount:module:<module qname>#urlpatterns>/<path>`
//! ([`glia_code_domain::endpoint::mount_route_qname`], [`Mount::Module`]),
//! with a POSITION at its call, and records on the urlconf's MODULE one
//! [`NavFact::IncludeMount`] per `path("api/", include("plane.app.urls"))`
//! (target [`IncludeTarget::Spec`], prefix `api/`) and per `*issue_urls`
//! spread of an imported `urlpatterns` ([`IncludeTarget::Import`], prefix
//! `""`). Here each include resolves to a MODULE of the graph:
//!
//! - a Spec, a dotted path as written, by qname tail (`plane.app.urls` is the
//!   MODULE `<..>::plane::app::urls` or the package `<..>::plane::app::urls::__init__`);
//!   two candidates break by the longer common `::` prefix with the includer
//!   (one project's urlconf), a tie or none is `unresolved`;
//! - an Import, through the includer's own import statement binding that
//!   name: a relative one exactly, as the import pass reads it (`from .issue
//!   import urlpatterns as issue_urls` in `app::urls::__init__` names
//!   `app::urls::issue`), an absolute one by qname tail.
//!
//! Then the fixpoint: each resolved include is a flow into the target's
//! module slot of the includer's own prefixes joined with the include
//! literal; a urlconf no resolved include reaches is a ROOT (Django's
//! ROOT_URLCONF, which is not read: in a sane project it is the one such
//! module) and holds the prefix `""`. A cycle, or a slot past
//! [`go_mounts::MAX_PREFIXES`], stops as the Go pass does (`capped`). Each
//! provisional ROUTE is re-keyed to `<METHOD> /<prefix><path>` once per
//! prefix of its module's slot (the first by rename, each further one a
//! clone, [`go_mounts::rekey`]); one whose module no root reaches is re-keyed
//! to `<METHOD> /<path>`. A Dynamic include (`include(router.urls)`) is
//! counted and mounts nothing. Structural identity only: no value flows.
//!
//! GD.41, Python second: Flask and FastAPI. A slot is `(module, router
//! variable)`: the Python parser mints a decorator route on a module-level
//! router (`router = APIRouter(..)`, `bp = Blueprint(..)`, a router subclass
//! such as `PrefectRouter(prefix="/flows")`) as `<METHOD>
//! <mount:module:<module>#<var>>/<path>` (a FastAPI path carrying the
//! router's own prefix, a Flask one only its rule) and records on the MODULE
//! one [`NavFact::IncludeMount`] per router variable (`via` `router:<family>`,
//! target `Dynamic(<var>)`, prefix its own: names the slot, mounts nothing)
//! and one per `<x>.include_router(<ref>, prefix=..)` (`via` `fastapi:<x>`,
//! prefix `x`'s own prefix then the include's) or `<x>.register_blueprint(
//! <ref>, url_prefix=..)` (`via` `flask:<x>`, or `flask_own:<x>` with no
//! `url_prefix`: the blueprint's own applies, Flask's override rule). An
//! Import `<ref>` resolves through [`ModuleIndex::router_import`]: a router
//! variable of the includer, an imported name (its module through the import
//! statement, its variable the imported name) or a module attribute
//! (`users.router`). The flow runs from the includer's slot `(module, x)`
//! into the target's; a slot no resolved include reaches is a root holding
//! `""`, a Flask blueprint root its own `url_prefix`.
//!
//! Deterministic: route slots and facts in `g.nodes` order of their MODULE
//! (facts in record order), roots by slot, BTree slots; the clones append in
//! provisional order.
//!
//! HQ.5, client-router pages ([`bind_nav_mounts`]): every object route table
//! of a TS-family file mints its pages on the file's provisional nav slot
//! (`page:<mount:module:<module>#nav>/<path>`,
//! [`glia_code_domain::endpoint::nav_mount_route_qname`]) and records each
//! Angular `loadChildren` as a [`NavFact::IncludeMount`] (`via` `nav_lazy:..`
//! / `nav_lazy_string:..`, prefix the loading record's own path); the same
//! fixpoint composes each lazily loaded table under its parent's path, and
//! a table no mount reaches keeps its own.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use glia_code_domain::endpoint::{
    abs_path, join_path, mount_route_qname, page_qname, parse_mount_route_qname,
    parse_nav_mount_link, parse_nav_mount_route_qname, split_owner, with_owner,
};
use glia_code_domain::{
    CallQualifier, GRAPH_TYPE, ImportStmt, ImportTarget, IncludeTarget, Mount, NavFact,
    UnresolvedRef, edge_category, node_kind,
};
use glia_core::{Cell, Confidence, Edge, NodeId};

use crate::go_mounts::{self, Final, Flow, Slot, collect_provisionals, fixpoint, rekey};
use crate::types::RepoGraph;

/// The router value a Django urlconf registers its routes in, and the one an
/// include mounts.
const DJANGO_VAR: &str = "urlpatterns";

/// The `via` of a spread [`NavFact::IncludeMount`].
const VIA_SPREAD: &str = "spread";

/// What [`bind_module_mounts`] did to one graph; [`Self::marker`] prints it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct RouteMountStats {
    /// Include facts naming a module (Spec or Import), `via` include.
    pub(crate) includes: usize,
    /// Spread facts naming a module.
    pub(crate) spreads: usize,
    /// Include or spread facts naming anything else: mount nothing.
    pub(crate) dynamic: usize,
    /// Of `includes + spreads`, the ones no MODULE of the graph answers.
    pub(crate) unresolved: usize,
    /// Route modules: those holding a provisional ROUTE or a mount fact.
    pub(crate) modules: usize,
    /// Provisional module ROUTEs found.
    pub(crate) routes: usize,
    /// Of those, the ones whose module a resolved include reaches and whose
    /// slot received a prefix.
    pub(crate) mounted: usize,
    /// ROUTEs cloned for a slot's second and later prefixes.
    pub(crate) copies: usize,
    /// Route modules no resolved include reaches: each holds the prefix `""`.
    pub(crate) roots: usize,
    /// Slots the fixpoint's caps stopped.
    pub(crate) capped: usize,
    /// GD.41: mount facts by framework: `register_blueprint` (Flask),
    /// `include_router` (FastAPI), `include` / spread (Django).
    pub(crate) via_flask: usize,
    pub(crate) via_fastapi: usize,
    pub(crate) via_django: usize,
}

impl RouteMountStats {
    /// `[route-mounts] lang=<l> includes=<i> spreads=<s> dynamic=<d>
    /// via(flask=<f> fastapi=<a> django=<j>) unresolved=<u> modules=<m>
    /// routes=<r> mounted=<k> copies=<c> roots=<o> capped=<x>`, once per
    /// graph with a route module (a route slot: a router variable, an
    /// includer, a provisional ROUTE's).
    pub(crate) fn marker(&self, lang: &str) -> Option<String> {
        (self.modules > 0).then(|| {
            format!(
                "[route-mounts] lang={lang} includes={} spreads={} dynamic={} \
                 via(flask={} fastapi={} django={}) unresolved={} \
                 modules={} routes={} mounted={} copies={} roots={} capped={}",
                self.includes,
                self.spreads,
                self.dynamic,
                self.via_flask,
                self.via_fastapi,
                self.via_django,
                self.unresolved,
                self.modules,
                self.routes,
                self.mounted,
                self.copies,
                self.roots,
                self.capped
            )
        })
    }
}

/// Re-key every provisional module-mount ROUTE of `g` (see the module doc).
/// `refs` are the graph's pending refs (a re-keyed ROUTE's move with it, a
/// clone gets a copy); `imports` the graph's import statements, which an
/// [`IncludeTarget::Import`] reads. A no-op on a graph with no route module.
pub(crate) fn bind_module_mounts(
    g: &mut RepoGraph,
    refs: &mut Vec<UnresolvedRef>,
    imports: &[ImportStmt],
) -> RouteMountStats {
    let mut stats = RouteMountStats::default();
    let provisionals: Vec<go_mounts::Provisional> = collect_provisionals(g, refs)
        .into_iter()
        .filter(|p| matches!(p.base, Mount::Module { .. }))
        .collect();

    // Route slots in `g.nodes` order of their MODULE (each module's facts in
    // record order), then the provisionals' own; the facts beside them.
    let mut route_slots: Vec<SlotKey> = Vec::new();
    let mut seen: BTreeSet<SlotKey> = BTreeSet::new();
    let mut facts: Vec<(String, Fact<'_>)> = Vec::new();
    let mut modules: Vec<(String, NodeId)> = Vec::new();
    // GD.41: each module's router variables, with their family and own prefix.
    let mut routers: BTreeMap<SlotKey, (String, String)> = BTreeMap::new();
    for n in &g.nodes {
        if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
            continue;
        }
        let Some(q) = g.nav.qname_by_id.get(&n.id) else {
            continue;
        };
        modules.push((q.clone(), n.id));
        let Some(fs) = g.nav.nav_facts.get(&n.id) else {
            continue;
        };
        for f in fs {
            let Some(fact) = Fact::read(f) else {
                continue;
            };
            let slot = (q.clone(), fact.includer.clone());
            if let (Some(family), IncludeTarget::Dynamic(var)) = (&fact.router, fact.target) {
                let slot = (q.clone(), var.clone());
                routers.insert(slot.clone(), (family.clone(), fact.prefix.to_string()));
                if seen.insert(slot.clone()) {
                    route_slots.push(slot);
                }
                continue;
            }
            if seen.insert(slot.clone()) {
                route_slots.push(slot);
            }
            facts.push((q.clone(), fact));
        }
    }
    for p in &provisionals {
        if let Mount::Module { module, var, .. } = &p.base {
            let slot = (module.clone(), var.clone());
            if seen.insert(slot.clone()) {
                route_slots.push(slot);
            }
        }
    }
    if route_slots.is_empty() {
        return stats;
    }
    let route_modules: BTreeSet<&str> = route_slots.iter().map(|(m, _)| m.as_str()).collect();
    stats.modules = route_modules.len();
    stats.routes = provisionals.len();
    modules.sort_by(|a, b| a.0.cmp(&b.0));

    // Each include resolved to a flow into its target's slot.
    let index = ModuleIndex { g, modules };
    let mut includes: Vec<Flow> = Vec::new();
    let mut included: BTreeSet<SlotKey> = BTreeSet::new();
    for (includer, fact) in &facts {
        match fact.family {
            Family::Django => stats.via_django += 1,
            Family::FastApi => stats.via_fastapi += 1,
            Family::Flask | Family::FlaskOwn => stats.via_flask += 1,
        }
        let resolved: Option<SlotKey> = match fact.target {
            IncludeTarget::Dynamic(_) => {
                stats.dynamic += 1;
                continue;
            }
            IncludeTarget::Spec(dotted) => index
                .by_tail(&dotted.replace('.', "::"), includer)
                .map(|m| (m, DJANGO_VAR.to_string())),
            IncludeTarget::Import { local } if fact.family == Family::Django => index
                .by_import(imports, includer, local)
                .map(|m| (m, DJANGO_VAR.to_string())),
            IncludeTarget::Import { local } => {
                index.router_import(imports, includer, local, &routers)
            }
        };
        if fact.via == VIA_SPREAD {
            stats.spreads += 1;
        } else {
            stats.includes += 1;
        }
        let Some(target) = resolved else {
            stats.unresolved += 1;
            continue;
        };
        // A Flask register with no `url_prefix` mounts the blueprint at its
        // own (FlaskOwn); any other include at its literal.
        let prefix = match fact.family {
            Family::FlaskOwn => routers
                .get(&target)
                .map(|(_, own)| own.clone())
                .unwrap_or_default(),
            _ => fact.prefix.to_string(),
        };
        let source = (includer.clone(), fact.includer.clone());
        if target != source {
            included.insert(target.clone());
        }
        includes.push(Flow {
            target: Slot::Module(target.0, target.1),
            mount: Mount::Module {
                module: source.0,
                var: source.1,
                suffix: prefix,
            },
        });
    }

    // Roots first (by slot), then the includes in fact order. A root holds
    // `""`, a Flask blueprint root its own `url_prefix` (its routes carry
    // only their rule).
    let roots: BTreeSet<&SlotKey> = route_slots
        .iter()
        .filter(|s| !included.contains(*s))
        .collect();
    stats.roots = roots.len();
    let mut flows: Vec<Flow> = roots
        .into_iter()
        .map(|s| {
            let own = match routers.get(s) {
                Some((family, own)) if family == FAMILY_FLASK => own.clone(),
                _ => String::new(),
            };
            Flow {
                target: Slot::Module(s.0.clone(), s.1.clone()),
                mount: Mount::Const(own),
            }
        })
        .collect();
    flows.extend(includes);
    let (slots, capped) = fixpoint(&flows, &BTreeMap::new());
    stats.capped = capped;

    let mut plans: Vec<Vec<Final>> = Vec::with_capacity(provisionals.len());
    for p in &provisionals {
        let slot: SlotKey = match &p.base {
            Mount::Module { module, var, .. } => (module.clone(), var.clone()),
            _ => (String::new(), String::new()),
        };
        let prefixes = slots
            .get(&Slot::Module(slot.0.clone(), slot.1.clone()))
            .filter(|s| !s.is_empty());
        let paths: BTreeSet<String> = match prefixes {
            Some(set) => {
                if included.contains(&slot) {
                    stats.mounted += 1;
                }
                set.iter()
                    .map(|m| abs_path(&join_path(m, &p.path)))
                    .collect()
            }
            None => BTreeSet::from([abs_path(&p.path)]),
        };
        stats.copies += paths.len().saturating_sub(1);
        plans.push(
            paths
                .iter()
                .map(|path| Final::new(g, &p.method, path, p.owner.as_deref()))
                .collect(),
        );
    }
    rekey(g, refs, &provisionals, &plans);
    stats
}

/// A mount slot: `(module qname, router variable)`.
type SlotKey = (String, String);

/// GD.41: the framework family of a router declaration fact.
const FAMILY_FLASK: &str = "flask";

/// Which mount an [`NavFact::IncludeMount`] is (its `via`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    /// GD.40: `include` / `spread`, slot var `urlpatterns`.
    Django,
    /// GD.41: `fastapi:<includer>`, an `include_router`.
    FastApi,
    /// GD.41: `flask:<includer>`, a `register_blueprint` with `url_prefix`.
    Flask,
    /// GD.41: `flask_own:<includer>`, one without: the blueprint's own.
    FlaskOwn,
}

/// One [`NavFact::IncludeMount`] read: its family, the includer's slot
/// variable and, for a GD.41 router declaration (`via` `router:<family>`,
/// target `Dynamic(<var>)`), the router's family.
struct Fact<'f> {
    family: Family,
    via: &'f str,
    includer: String,
    target: &'f IncludeTarget,
    prefix: &'f str,
    router: Option<String>,
}

impl<'f> Fact<'f> {
    fn read(f: &'f NavFact) -> Option<Fact<'f>> {
        let NavFact::IncludeMount {
            target,
            prefix,
            via,
            ..
        } = f
        else {
            return None;
        };
        let (family, includer, router) = match via.split_once(':') {
            Some(("router", family)) => (Family::FastApi, String::new(), Some(family.to_string())),
            Some(("fastapi", x)) => (Family::FastApi, x.to_string(), None),
            Some(("flask", x)) => (Family::Flask, x.to_string(), None),
            Some(("flask_own", x)) => (Family::FlaskOwn, x.to_string(), None),
            _ => (Family::Django, DJANGO_VAR.to_string(), None),
        };
        Some(Fact {
            family,
            via,
            includer,
            target,
            prefix,
            router,
        })
    }
}

/// The graph's MODULEs by qname, for include targets.
struct ModuleIndex<'g> {
    g: &'g RepoGraph,
    /// `(qname, id)` of every MODULE, sorted by qname.
    modules: Vec<(String, NodeId)>,
}

impl ModuleIndex<'_> {
    /// The MODULE `qname`, or the package `qname::__init__` (the import
    /// pass's order: the module first).
    fn exact(&self, qname: &str) -> Option<String> {
        let table = &self.g.symbols.module_by_qname;
        if table.contains_key(qname) {
            return Some(qname.to_string());
        }
        let init = format!("{qname}::__init__");
        table.contains_key(&init).then_some(init)
    }

    /// The MODULE a dotted path names by qname tail (see the module doc):
    /// `tail` itself, or any `<..>::tail`, each as a module or a package
    /// `__init__`. Several candidates: the exact one, else the ones sharing
    /// the longest `::` prefix with `includer`, kept only when one remains.
    fn by_tail(&self, tail: &str, includer: &str) -> Option<String> {
        if tail.is_empty() {
            return None;
        }
        if let Some(q) = self.exact(tail) {
            return Some(q);
        }
        let suffix = format!("::{tail}");
        // Candidates by package path (the qname without `::__init__`).
        let mut by_path: BTreeMap<&str, &str> = BTreeMap::new();
        for (q, _) in &self.modules {
            let path = q.strip_suffix("::__init__").unwrap_or(q);
            if path.ends_with(&suffix) {
                // The module before the package, as `exact` reads them.
                let e = by_path.entry(path).or_insert(q);
                if q.as_str() == path {
                    *e = q;
                }
            }
        }
        let shared = |path: &str| {
            path.split("::")
                .zip(includer.split("::"))
                .take_while(|(a, b)| a == b)
                .count()
        };
        let best = by_path.keys().map(|p| shared(p)).max()?;
        let mut picks = by_path.iter().filter(|(p, _)| shared(p) == best);
        match (picks.next(), picks.next()) {
            (Some((_, q)), None) => Some((*q).to_string()),
            _ => None,
        }
    }

    /// The MODULE the name `local` stands for in module `includer`, through
    /// the last import statement of `includer` binding it: `import a.urls as
    /// local` names `a.urls`; `from <m> import <name> [as local]` names the
    /// submodule `<m>.<name>` when there is one, else `<m>` (whose
    /// `<name>`, `urlpatterns`, the spread reads). A relative `<m>` resolves
    /// exactly against `includer`, an absolute one by qname tail.
    fn by_import(&self, imports: &[ImportStmt], includer: &str, local: &str) -> Option<String> {
        let stmt = imports.iter().rev().find(|s| {
            s.from_module == includer
                && match &s.target {
                    ImportTarget::Module { path, alias } => {
                        alias.as_deref().unwrap_or(path.as_str()) == local
                    }
                    ImportTarget::Symbol { name, alias, .. } => {
                        alias.as_deref().unwrap_or(name.as_str()) == local
                    }
                }
        })?;
        match &stmt.target {
            ImportTarget::Module { path, .. } => self.by_tail(&path.replace('.', "::"), includer),
            ImportTarget::Symbol {
                module,
                name,
                level,
                ..
            } => {
                if *level > 0 {
                    let base = relative_module(includer, module, *level);
                    let sub = if base.is_empty() {
                        name.clone()
                    } else {
                        format!("{base}::{name}")
                    };
                    self.exact(&sub).or_else(|| self.exact(&base))
                } else {
                    let base = module.replace('.', "::");
                    self.by_tail(&format!("{base}::{name}"), includer)
                        .or_else(|| self.by_tail(&base, includer))
                }
            }
        }
    }
}

impl ModuleIndex<'_> {
    /// GD.41: the router slot an include's `local` names in module
    /// `includer`: a router variable `includer` itself declares (`router`),
    /// a name `includer` imports (`from routers.users import router [as r]`:
    /// the module through [`Self::by_import`], the variable the imported
    /// name), or an imported module's attribute (`users.router`: the module
    /// `users` stands for, variable `router`). An imported name that is
    /// itself a module (`from routers import users`) is no router: `None`.
    fn router_import(
        &self,
        imports: &[ImportStmt],
        includer: &str,
        local: &str,
        routers: &BTreeMap<SlotKey, (String, String)>,
    ) -> Option<SlotKey> {
        if let Some((object, var)) = local.rsplit_once('.') {
            let module = self.by_import(imports, includer, object)?;
            return Some((module, var.to_string()));
        }
        let own = (includer.to_string(), local.to_string());
        if routers.contains_key(&own) {
            return Some(own);
        }
        let stmt = imports.iter().rev().find(|s| {
            s.from_module == includer
                && matches!(&s.target, ImportTarget::Symbol { name, alias, .. }
                    if alias.as_deref().unwrap_or(name.as_str()) == local)
        })?;
        let ImportTarget::Symbol { name, .. } = &stmt.target else {
            return None;
        };
        let module = self.by_import(imports, includer, local)?;
        let path = module.strip_suffix("::__init__").unwrap_or(&module);
        let named_module = path.rsplit("::").next() == Some(name.as_str())
            && !routers.contains_key(&(module.clone(), name.clone()));
        (!named_module).then(|| (module.clone(), name.clone()))
    }
}

/// The module qname a relative `from <dots><module_ref> import ..` in
/// `from_module` names: `level` trailing components popped, then
/// `module_ref`. The rule the import pass reads relative imports by
/// (`imports::resolve_module_reference`), for a statement it already saw.
fn relative_module(from_module: &str, module_ref: &str, level: u32) -> String {
    let mut parts: Vec<&str> = from_module.split("::").collect();
    for _ in 0..level {
        parts.pop();
    }
    let base = parts.join("::");
    let rel = module_ref.replace('.', "::");
    match (base.is_empty(), rel.is_empty()) {
        (_, true) => base,
        (true, false) => rel,
        (false, false) => format!("{base}::{rel}"),
    }
}

// ============================================================================
// GD.42: TypeScript / JavaScript
// ============================================================================

/// What [`bind_ts_mounts`] did to one TypeScript-family graph;
/// [`Self::marker`] prints it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct TsMountStats {
    /// Mount facts naming a router slot of the graph (each flows).
    pub(crate) includes: usize,
    /// Mount facts whose target resolved to something that is no router (a
    /// middleware, a module exporting no router): mount nothing.
    pub(crate) dynamic: usize,
    /// Mount facts whose target no MODULE of the graph answers.
    pub(crate) unresolved: usize,
    /// Mount facts by framework: `.use(..)` (Express), `.route(..)` (Hono),
    /// `.use(.., <sub>.routes())` (Koa), `setGlobalPrefix` calls, the
    /// `RouterModule.register` entries, `@Module` controller lists.
    pub(crate) via_express: usize,
    pub(crate) via_hono: usize,
    pub(crate) via_koa: usize,
    pub(crate) nest_global: usize,
    pub(crate) nest_router: usize,
    pub(crate) nest_modules: usize,
    /// Route modules: those holding a router slot or a provisional ROUTE.
    pub(crate) modules: usize,
    /// Provisional module ROUTEs found (NestJS controller routes re-keyed to
    /// their slot included).
    pub(crate) routes: usize,
    /// Of those, the ones a resolved mount reaches or a global prefix moved.
    pub(crate) mounted: usize,
    /// ROUTEs cloned for a slot's second and later prefixes.
    pub(crate) copies: usize,
    /// Router slots no resolved mount reaches: each holds the prefix `""`.
    pub(crate) roots: usize,
    /// Slots the fixpoint's caps stopped.
    pub(crate) capped: usize,
    /// NestJS routes a global prefix's `exclude` kept at their own path.
    pub(crate) excluded: usize,
}

impl TsMountStats {
    /// `[route-mounts] lang=typescript includes=<i> dynamic=<d>
    /// via(express=<e> hono=<h> koa=<k> nest_global=<g> nest_router=<r>)
    /// nest_modules=<n> unresolved=<u> modules=<m> routes=<r> mounted=<k>
    /// copies=<c> roots=<o> capped=<x> excluded=<y>`, once per graph with a
    /// route module.
    pub(crate) fn marker(&self) -> Option<String> {
        (self.modules > 0).then(|| {
            format!(
                "[route-mounts] lang=typescript includes={} dynamic={} \
                 via(express={} hono={} koa={} nest_global={} nest_router={}) \
                 nest_modules={} unresolved={} modules={} routes={} mounted={} \
                 copies={} roots={} capped={} excluded={}",
                self.includes,
                self.dynamic,
                self.via_express,
                self.via_hono,
                self.via_koa,
                self.nest_global,
                self.nest_router,
                self.nest_modules,
                self.unresolved,
                self.modules,
                self.routes,
                self.mounted,
                self.copies,
                self.roots,
                self.capped,
                self.excluded
            )
        })
    }
}

/// The router family of a NestJS controller slot.
const FAMILY_NEST: &str = "nest";

/// GD.42: re-key every provisional module-mount ROUTE of a TypeScript-family
/// graph, recorded by the `ts_routes` extractor (see its `MountScan`):
///
/// - Express / Koa / Hono: a route on a module router `<x>` is minted on the
///   slot `(module, x)`; each `.use('/p', <ref>)` / `.route('/p', <ref>)`
///   on `<x>` flows `x`'s prefixes joined with `/p` into the slot `<ref>`
///   names, resolved here: a router of the includer itself, an imported name
///   (through the import pass's binding: a MODULE names its default-exported
///   router, any other node its own module and name; else through the
///   includer's import statement and `resolve_source`), a module attribute
///   (`users.router`), or `require('<spec>')` (the module's default export,
///   through `resolve_source`). A target that is no router slot (a
///   middleware) mounts nothing (`dynamic`); one no MODULE answers is
///   `unresolved`.
/// - NestJS: when the graph holds a `setGlobalPrefix` or a `RouterModule`
///   entry, each controller route (a plain ROUTE whose module's facts name
///   its controller by its first POSITION row) is first renamed onto the
///   slot `(module, Controller)`; an `@Module({ controllers })` flows the
///   module's prefixes into each controller's slot, a `RouterModule` entry
///   its path into the module's. Then each controller route takes the global
///   prefix of the `setGlobalPrefix` nearest its module (the one sharing the
///   longest `::` prefix with it: one Nest app's main file; two at that
///   length with different prefixes give none), unless the prefix's
///   `exclude` names its path (a literal, a `{ path, method }`, or a path
///   list constant, here or imported; a trailing `*` / `(.*)` matches below).
///
/// A slot no resolved mount reaches is a root holding `""`; the fixpoint,
/// caps and re-key are GD.40's. Deterministic as [`bind_module_mounts`].
pub(crate) fn bind_ts_mounts<R>(
    g: &mut RepoGraph,
    refs: &mut Vec<UnresolvedRef>,
    imports: &[ImportStmt],
    resolve_source: &R,
) -> TsMountStats
where
    R: Fn(&str, &str) -> Option<String>,
{
    let mut stats = TsMountStats::default();
    let facts = TsFacts::read(g);
    if facts.is_empty() {
        return stats;
    }
    if !facts.globals.is_empty()
        || facts
            .includes
            .iter()
            .any(|i| i.family == TsFamily::NestRouter)
    {
        facts.rename_nest_routes(g, refs);
    }
    let provisionals: Vec<go_mounts::Provisional> = collect_provisionals(g, refs)
        .into_iter()
        .filter(|p| matches!(p.base, Mount::Module { .. }))
        .collect();

    let mut route_slots: BTreeSet<SlotKey> = facts.routers.keys().cloned().collect();
    for i in &facts.includes {
        route_slots.insert((i.module.clone(), i.includer.clone()));
    }
    for p in &provisionals {
        if let Mount::Module { module, var, .. } = &p.base {
            route_slots.insert((module.clone(), var.clone()));
        }
    }
    if route_slots.is_empty() {
        return stats;
    }
    stats.modules = route_slots
        .iter()
        .map(|(m, _)| m.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    stats.routes = provisionals.len();

    let resolver = TsResolver {
        g,
        imports,
        resolve_source,
        facts: &facts,
    };
    let mut flows: Vec<Flow> = Vec::new();
    let mut included: BTreeSet<SlotKey> = BTreeSet::new();
    for i in &facts.includes {
        match i.family {
            TsFamily::Express => stats.via_express += 1,
            TsFamily::Hono => stats.via_hono += 1,
            TsFamily::Koa => stats.via_koa += 1,
            TsFamily::NestRouter => stats.nest_router += 1,
            TsFamily::NestModule => stats.nest_modules += 1,
        }
        let target = match resolver.target(&i.module, &i.target) {
            Target::Slot(t) if facts.routers.contains_key(&t) => t,
            Target::Slot(_) | Target::NotRouter => {
                stats.dynamic += 1;
                continue;
            }
            Target::Unresolved => {
                stats.unresolved += 1;
                continue;
            }
        };
        stats.includes += 1;
        let source = (i.module.clone(), i.includer.clone());
        if target != source {
            included.insert(target.clone());
        }
        flows.push(Flow {
            target: Slot::Module(target.0, target.1),
            mount: Mount::Module {
                module: source.0,
                var: source.1,
                suffix: i.prefix.clone(),
            },
        });
    }
    stats.nest_global = facts.globals.len();
    let globals: Vec<Global> = facts
        .globals
        .iter()
        .map(|gl| {
            let mut excludes = gl.excludes.clone();
            for local in &gl.refs {
                if let Some(slot) = resolver.name(&gl.module, local)
                    && let Some(paths) = facts.path_lists.get(&slot)
                {
                    excludes.extend(paths.iter().map(|p| ("ANY".to_string(), p.clone())));
                }
            }
            (gl.module.clone(), gl.prefix.clone(), excludes)
        })
        .collect();

    let roots: Vec<&SlotKey> = route_slots
        .iter()
        .filter(|s| !included.contains(*s))
        .collect();
    stats.roots = roots.len();
    let mut all: Vec<Flow> = roots
        .into_iter()
        .map(|s| Flow {
            target: Slot::Module(s.0.clone(), s.1.clone()),
            mount: Mount::Const(String::new()),
        })
        .collect();
    all.extend(flows);
    let (slots, capped) = fixpoint(&all, &BTreeMap::new());
    stats.capped = capped;

    let mut plans: Vec<Vec<Final>> = Vec::with_capacity(provisionals.len());
    for p in &provisionals {
        let slot: SlotKey = match &p.base {
            Mount::Module { module, var, .. } => (module.clone(), var.clone()),
            _ => (String::new(), String::new()),
        };
        let prefixes = slots
            .get(&Slot::Module(slot.0.clone(), slot.1.clone()))
            .filter(|s| !s.is_empty());
        let mut moved = prefixes.is_some() && included.contains(&slot);
        let mut paths: BTreeSet<String> = match prefixes {
            Some(set) => set
                .iter()
                .map(|m| abs_path(&join_path(m, &p.path)))
                .collect(),
            None => BTreeSet::from([abs_path(&p.path)]),
        };
        if facts.routers.get(&slot).map(String::as_str) == Some(FAMILY_NEST)
            && let Some((prefix, excludes)) = nearest_global(&globals, &slot.0)
        {
            paths = paths
                .into_iter()
                .map(|path| {
                    if nest_excluded(&excludes, &p.method, &path) {
                        stats.excluded += 1;
                        path
                    } else {
                        moved = true;
                        abs_path(&join_path(prefix, &path))
                    }
                })
                .collect();
        }
        if moved {
            stats.mounted += 1;
        }
        stats.copies += paths.len().saturating_sub(1);
        plans.push(
            paths
                .iter()
                .map(|path| Final::new(g, &p.method, path, p.owner.as_deref()))
                .collect(),
        );
    }
    rekey(g, refs, &provisionals, &plans);
    stats
}

/// The global prefix of the `setGlobalPrefix` nearest `module` (the most
/// `::` segments shared from the start) with the excludes of every call at
/// that distance; `None` with no global, or two at that distance with
/// different prefixes.
fn nearest_global<'a>(globals: &'a [Global], module: &str) -> Option<(&'a str, Vec<Exclude>)> {
    let shared = |m: &str| {
        m.split("::")
            .zip(module.split("::"))
            .take_while(|(a, b)| a == b)
            .count()
    };
    let best = globals.iter().map(|(m, _, _)| shared(m)).max()?;
    let picks: Vec<_> = globals
        .iter()
        .filter(|(m, _, _)| shared(m) == best)
        .collect();
    let prefix = picks.first()?.1.as_str();
    if picks.iter().any(|(_, p, _)| p != prefix) {
        return None;
    }
    let excludes = picks
        .iter()
        .flat_map(|(_, _, e)| e.iter().cloned())
        .collect();
    Some((prefix, excludes))
}

/// An exclude of a NestJS global prefix: `(METHOD | ANY, path)`.
type Exclude = (String, String);

/// A `setGlobalPrefix` with its excludes resolved: `(module, prefix,
/// excludes)`.
type Global = (String, String, Vec<Exclude>);

/// Whether a NestJS global prefix's `excludes` (`(METHOD | ANY, path)`)
/// name the route `method path`: the paths compared without their outer
/// `/`, a trailing `*`, `(.*)` or `{*splat}` matching below.
fn nest_excluded(excludes: &[Exclude], method: &str, path: &str) -> bool {
    let route = path.trim_matches('/');
    excludes.iter().any(|(m, ex)| {
        if m != "ANY" && m != method {
            return false;
        }
        let ex = ex.trim_matches('/');
        for wild in ["(.*)", "{*splat}", "*"] {
            if let Some(base) = ex.strip_suffix(wild) {
                return route.starts_with(base.trim_end_matches('/'));
            }
        }
        ex == route
    })
}

/// Which mount a TS [`NavFact::IncludeMount`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TsFamily {
    Express,
    Koa,
    Hono,
    /// An `@Module` controller list: includer the module class.
    NestModule,
    /// A `RouterModule.register` entry: includer `RouterModule`.
    NestRouter,
}

/// One mount fact: the includer slot `(module, includer)`, its target and
/// its prefix (the includer's own already joined in).
struct TsInclude {
    module: String,
    includer: String,
    family: TsFamily,
    target: IncludeTarget,
    prefix: String,
}

/// One `setGlobalPrefix`: its file MODULE, prefix and excludes (literal
/// `(METHOD | ANY, path)` pairs, and named path lists to resolve).
struct NestGlobal {
    module: String,
    prefix: String,
    excludes: Vec<(String, String)>,
    refs: Vec<String>,
}

/// The `ts_routes` mount facts of a graph, read in `g.nodes` order of their
/// MODULE, each module's in record order.
#[derive(Default)]
struct TsFacts {
    /// Router slots, by family (`express`, `koa`, `hono`, `nest`,
    /// `nest_module`).
    routers: BTreeMap<SlotKey, String>,
    /// Module -> its default-exported router variable (the first recorded).
    defaults: BTreeMap<String, String>,
    includes: Vec<TsInclude>,
    /// `(module, 0-based row)` of a NestJS route -> its controller class.
    nest_routes: BTreeMap<(String, u32), String>,
    globals: Vec<NestGlobal>,
    /// `(module, constant)` -> the paths of a path-list constant.
    path_lists: BTreeMap<SlotKey, Vec<String>>,
}

impl TsFacts {
    fn read(g: &RepoGraph) -> TsFacts {
        let mut out = TsFacts::default();
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                continue;
            }
            let (Some(m), Some(fs)) = (g.nav.qname_by_id.get(&n.id), g.nav.nav_facts.get(&n.id))
            else {
                continue;
            };
            for f in fs {
                let NavFact::IncludeMount {
                    line,
                    target,
                    prefix,
                    via,
                } = f
                else {
                    continue;
                };
                out.read_one(m, *line, target, prefix, via);
            }
        }
        out
    }

    fn read_one(&mut self, m: &str, line: u32, target: &IncludeTarget, prefix: &str, via: &str) {
        let include = |family: TsFamily, includer: &str| TsInclude {
            module: m.to_string(),
            includer: includer.to_string(),
            family,
            target: target.clone(),
            prefix: prefix.to_string(),
        };
        let dynamic = match target {
            IncludeTarget::Dynamic(v) => Some(v.as_str()),
            _ => None,
        };
        match (via.split_once(':'), dynamic) {
            (Some(("router", family)), Some(var)) => {
                self.routers
                    .insert((m.to_string(), var.to_string()), family.to_string());
            }
            (Some(("express", x)), _) => self.includes.push(include(TsFamily::Express, x)),
            (Some(("koa", x)), _) => self.includes.push(include(TsFamily::Koa, x)),
            (Some(("hono", x)), _) => self.includes.push(include(TsFamily::Hono, x)),
            (Some(("nest_module", x)), _) => {
                self.includes.push(include(TsFamily::NestModule, x));
            }
            (Some(("path_list", x)), _) => {
                if let IncludeTarget::Spec(p) = target {
                    self.path_lists
                        .entry((m.to_string(), x.to_string()))
                        .or_default()
                        .push(p.clone());
                }
            }
            (Some(("nest_exclude", method)), _) => {
                if let (IncludeTarget::Spec(p), Some(gl)) = (target, self.globals.last_mut())
                    && gl.module == m
                {
                    gl.excludes.push((method.to_string(), p.clone()));
                }
            }
            (None, Some(var)) if via == "export_default" => {
                self.defaults
                    .entry(m.to_string())
                    .or_insert_with(|| var.to_string());
            }
            (None, Some(class)) if via == "nest_route" => {
                self.nest_routes
                    .insert((m.to_string(), line), class.to_string());
            }
            (None, _) if via == "nest_router" => {
                self.includes
                    .push(include(TsFamily::NestRouter, "RouterModule"));
            }
            (None, _) if via == "nest_global" => self.globals.push(NestGlobal {
                module: m.to_string(),
                prefix: prefix.to_string(),
                excludes: Vec::new(),
                refs: Vec::new(),
            }),
            (None, _) if via == "nest_exclude_ref" => {
                if let (IncludeTarget::Import { local }, Some(gl)) =
                    (target, self.globals.last_mut())
                    && gl.module == m
                {
                    gl.refs.push(local.clone());
                }
            }
            _ => {}
        }
    }

    fn is_empty(&self) -> bool {
        self.routers.is_empty() && self.includes.is_empty() && self.globals.is_empty()
    }

    /// Rename each NestJS controller ROUTE (a plain `<METHOD> <path>` one of
    /// whose POSITIONs is a row the facts of its file's MODULE name a
    /// controller at: a ROUTE a test file's request also minted carries that
    /// file's POSITION first) onto its controller's slot, so the mount
    /// fixpoint re-keys it like any other.
    fn rename_nest_routes(&self, g: &mut RepoGraph, refs: &mut [UnresolvedRef]) {
        if self.nest_routes.is_empty() {
            return;
        }
        let nest_modules: BTreeSet<&str> =
            self.nest_routes.keys().map(|(m, _)| m.as_str()).collect();
        // The file of each MODULE holding a controller route.
        let mut module_of_file: BTreeMap<String, String> = BTreeMap::new();
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                continue;
            }
            if let Some(q) = g.nav.qname_by_id.get(&n.id)
                && nest_modules.contains(q.as_str())
                && let Some((file, _)) = glia_code_domain::evidence::locate(&n.cells)
            {
                module_of_file.entry(file).or_insert_with(|| q.clone());
            }
        }
        let mut renames: Vec<(NodeId, NodeId)> = Vec::new();
        let mut names: Vec<(NodeId, String, String)> = Vec::new();
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ROUTE) {
                continue;
            }
            let Some((module, class)) = positions(&n.cells).into_iter().find_map(|(file, row)| {
                let module = module_of_file.get(&file)?;
                let class = self.nest_routes.get(&(module.clone(), row))?;
                Some((module.clone(), class.clone()))
            }) else {
                continue;
            };
            let Some(q) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            let (bare, owner) = split_owner(q);
            if parse_mount_route_qname(bare).is_some() {
                continue;
            }
            let Some((method, path)) = bare.split_once(' ') else {
                continue;
            };
            let mount = Mount::Module {
                module,
                var: class,
                suffix: String::new(),
            };
            let name = mount_route_qname(method, &mount, path);
            let qname = with_owner(&name, owner.unwrap_or(""));
            let id = NodeId::from_parts(GRAPH_TYPE, g.repo, node_kind::ROUTE, &qname);
            renames.push((n.id, id));
            names.push((id, qname, name));
        }
        let absorbed = super::rename_nodes(g, &mut [], refs, &renames);
        for ((old, _), (id, qname, name)) in renames.iter().zip(names) {
            if !absorbed.contains(old) {
                g.nav.qname_by_id.insert(id, qname);
                g.nav.name_by_id.insert(id, name);
            }
        }
    }
}

/// Every `(file, 0-based start_line)` of a node's POSITION cells, in cell
/// order.
fn positions(cells: &[glia_core::Cell]) -> Vec<(String, u32)> {
    cells
        .iter()
        .filter(|c| c.kind == glia_code_domain::cell_type::POSITION)
        .filter_map(
            |c| match glia_code_domain::evidence::locate(std::slice::from_ref(c))? {
                (file, Some(row)) => Some((file, row)),
                (_, None) => None,
            },
        )
        .collect()
}

/// What a mount target resolved to.
enum Target {
    /// A `(module, variable)` slot; a router when [`TsFacts::routers`] holds it.
    Slot(SlotKey),
    /// A module that exports no router.
    NotRouter,
    /// Nothing in the graph.
    Unresolved,
}

/// Resolves a TS mount target (see [`bind_ts_mounts`]).
struct TsResolver<'a, R> {
    g: &'a RepoGraph,
    imports: &'a [ImportStmt],
    resolve_source: &'a R,
    facts: &'a TsFacts,
}

impl<R: Fn(&str, &str) -> Option<String>> TsResolver<'_, R> {
    fn target(&self, includer: &str, target: &IncludeTarget) -> Target {
        match target {
            IncludeTarget::Dynamic(_) => Target::NotRouter,
            IncludeTarget::Spec(spec) => match self.spec_module(includer, spec) {
                Some(m) => self.default_of(m),
                None => Target::Unresolved,
            },
            IncludeTarget::Import { local } => {
                if let Some((object, var)) = local.split_once('.') {
                    return match self.module_named(includer, object) {
                        Some(m) => Target::Slot((m, var.to_string())),
                        None => Target::Unresolved,
                    };
                }
                let own = (includer.to_string(), local.clone());
                if self.facts.routers.contains_key(&own) {
                    return Target::Slot(own);
                }
                match self.binding(includer, local) {
                    Some(Bound::Module(m)) => self.default_of(m),
                    Some(Bound::Name(slot)) => Target::Slot(slot),
                    // An imported name nothing in the graph answers.
                    None if self.import_of(includer, local).is_some() => Target::Unresolved,
                    // Any other name is the includer's own, and no router
                    // (a middleware const).
                    None => Target::NotRouter,
                }
            }
        }
    }

    /// The `(module, name)` a name of `includer` stands for (a path-list
    /// constant): its own, else an imported one's declaration.
    fn name(&self, includer: &str, local: &str) -> Option<SlotKey> {
        let own = (includer.to_string(), local.to_string());
        if self.facts.path_lists.contains_key(&own) {
            return Some(own);
        }
        match self.binding(includer, local)? {
            Bound::Name(slot) => Some(slot),
            Bound::Module(_) => None,
        }
    }

    fn default_of(&self, module: String) -> Target {
        match self.facts.defaults.get(&module) {
            Some(var) => Target::Slot((module, var.clone())),
            None => Target::NotRouter,
        }
    }

    /// The MODULE a namespace-like name of `includer` binds (`import * as
    /// users`, `const users = require('./users')`).
    fn module_named(&self, includer: &str, object: &str) -> Option<String> {
        match self.binding(includer, object)? {
            Bound::Module(m) => Some(m),
            Bound::Name(_) => None,
        }
    }

    /// What `local` names in module `includer`: the import pass's binding
    /// (a MODULE, or a declaration's own module and name), else the last
    /// import statement of `includer` binding it, through `resolve_source`
    /// (a default import names the MODULE, a named one its name there).
    fn binding(&self, includer: &str, local: &str) -> Option<Bound> {
        let g = self.g;
        let from = g.symbols.module_by_qname.get(includer)?;
        if let Some(id) = g
            .symbols
            .module_import_bindings
            .get(from)
            .and_then(|b| b.get(local))
        {
            if g.nav.kind_by_id.get(id) == Some(&node_kind::MODULE) {
                return g.nav.qname_by_id.get(id).cloned().map(Bound::Module);
            }
            let name = g.nav.name_by_id.get(id)?;
            let mut at = *id;
            for _ in 0..32 {
                at = *g.nav.parent_of.get(&at)?;
                if g.nav.kind_by_id.get(&at) == Some(&node_kind::MODULE) {
                    let m = g.nav.qname_by_id.get(&at)?;
                    return Some(Bound::Name((m.clone(), name.clone())));
                }
            }
            return None;
        }
        let stmt = self.import_of(includer, local)?;
        match &stmt.target {
            ImportTarget::Module { path, .. } => {
                self.spec_module(includer, path).map(Bound::Module)
            }
            ImportTarget::Symbol { module, name, .. } => {
                let m = self.spec_module(includer, module)?;
                if name == "default" {
                    Some(Bound::Module(m))
                } else {
                    Some(Bound::Name((m, name.clone())))
                }
            }
        }
    }

    /// The last import statement of `includer` binding `local` (a
    /// namespace-like alias, or a named / default import).
    fn import_of(&self, includer: &str, local: &str) -> Option<&ImportStmt> {
        self.imports.iter().rev().find(|s| {
            s.from_module == includer
                && match &s.target {
                    ImportTarget::Module { alias, .. } => alias.as_deref() == Some(local),
                    ImportTarget::Symbol { name, alias, .. } => {
                        alias.as_deref().unwrap_or(name.as_str()) == local
                    }
                }
        })
    }

    /// The MODULE `spec` names from `includer`, through the builder's
    /// `resolve_source` (a workspace tag stripped): the module itself, else
    /// its directory's `index`.
    fn spec_module(&self, includer: &str, spec: &str) -> Option<String> {
        let raw = (self.resolve_source)(includer, spec)?;
        let q = raw
            .strip_prefix(crate::imports::WORKSPACE_ENTRY)
            .or_else(|| raw.strip_prefix(crate::imports::WORKSPACE_SUBPATH))
            .unwrap_or(&raw);
        if q.is_empty() {
            return None;
        }
        let table = &self.g.symbols.module_by_qname;
        if table.contains_key(q) {
            return Some(q.to_string());
        }
        let index = format!("{q}::index");
        table.contains_key(&index).then_some(index)
    }
}

/// What an imported name binds.
enum Bound {
    Module(String),
    Name(SlotKey),
}

// ============================================================================
// HQ.5: client-router lazy child tables
// ============================================================================

/// The router variable of a file's one nav mount slot.
const NAV_VAR: &str = "nav";

/// The `via` of an Angular `loadChildren: () => import(..)` mount fact (a
/// dynamic one included).
const VIA_NAV_LAZY: &str = "nav_lazy:";

/// The `via` of a legacy `loadChildren: '<spec>#<export>'` mount fact.
const VIA_NAV_LAZY_STRING: &str = "nav_lazy_string:";

/// What [`bind_nav_mounts`] did to one TS-family graph; [`Self::marker`]
/// prints it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct NavMountStats {
    /// `loadChildren` mount facts read (`import + string + dynamic`).
    pub(crate) lazy: usize,
    /// Of those, the `() => import('<spec>')` form.
    pub(crate) import: usize,
    /// The legacy `'<spec>#<export>'` string form.
    pub(crate) string: usize,
    /// Resolved through the NgModule one import hop on (`x.module` ->
    /// `x-routing.module`).
    pub(crate) ngmodule_hop: usize,
    /// A spec held in a variable or a template: mount nothing.
    pub(crate) dynamic: usize,
    /// A spec no MODULE of the graph answers: mounts nothing.
    pub(crate) unresolved: usize,
    /// A resolved module holding no route table, and no unique imported
    /// module holding one: mounts nothing.
    pub(crate) no_table: usize,
    /// Files holding a provisional nav page.
    pub(crate) tables: usize,
    /// Provisional nav pages found.
    pub(crate) routes: usize,
    /// Of those, the ones whose file a resolved mount reaches.
    pub(crate) mounted: usize,
    /// Pages cloned for a slot's second and later prefixes.
    pub(crate) copies: usize,
    /// Nav slots no resolved mount reaches: each holds the prefix `""`.
    pub(crate) roots: usize,
    /// Slots the fixpoint's caps stopped.
    pub(crate) capped: usize,
    /// Provisional redirect links rewritten to their final path.
    pub(crate) links_rewritten: usize,
}

impl NavMountStats {
    /// `[nav-mounts] lazy=<l> (import=<i> string=<s> ngmodule_hop=<h>
    /// dynamic=<d>) unresolved=<u> no_table=<t> tables=<n> routes=<r>
    /// mounted=<m> copies=<c> roots=<o> capped=<x> links_rewritten=<w>`, once
    /// per graph holding a provisional nav page.
    pub(crate) fn marker(&self) -> Option<String> {
        (self.routes > 0).then(|| {
            format!(
                "[nav-mounts] lazy={} (import={} string={} ngmodule_hop={} dynamic={}) \
                 unresolved={} no_table={} tables={} routes={} mounted={} copies={} \
                 roots={} capped={} links_rewritten={}",
                self.lazy,
                self.import,
                self.string,
                self.ngmodule_hop,
                self.dynamic,
                self.unresolved,
                self.no_table,
                self.tables,
                self.routes,
                self.mounted,
                self.copies,
                self.roots,
                self.capped,
                self.links_rewritten
            )
        })
    }
}

/// A provisional nav page: its slot's module, its path within the file's
/// tables, and what each clone copies (cells, incident edges, refs, as they
/// were before any re-key).
struct NavProvisional {
    id: NodeId,
    module: String,
    path: String,
    owner: Option<String>,
    confidence: Confidence,
    cells: Vec<Cell>,
    edges: Vec<Edge>,
    refs: Vec<UnresolvedRef>,
}

/// One final page of a provisional, and the prefix it was mounted at.
struct NavFinal {
    id: NodeId,
    qname: String,
    name: String,
    prefix: String,
}

impl NavFinal {
    fn new(g: &RepoGraph, path: String, prefix: String, owner: Option<&str>) -> NavFinal {
        let qname = with_owner(&page_qname(&path), owner.unwrap_or(""));
        let id = NodeId::from_parts(GRAPH_TYPE, g.repo, node_kind::ROUTE, &qname);
        NavFinal {
            id,
            qname,
            name: path,
            prefix,
        }
    }
}

/// A page path `path` (composed by its file's tables) under the mount prefix
/// `prefix` (no leading `/`; `""` for a root, which keeps `path` byte for
/// byte).
fn nav_join(prefix: &str, path: &str) -> String {
    if prefix.is_empty() {
        path.to_string()
    } else {
        abs_path(&join_path(prefix, path))
    }
}

/// HQ.5: re-key every provisional nav page of a TypeScript-family graph (see
/// the module doc) and rewrite every provisional redirect link:
///
/// 1. the provisional pages, in `g.nodes` order, with their incident edges
///    and pending refs;
/// 2. each `nav_lazy` / `nav_lazy_string` [`NavFact::IncludeMount`] of a
///    MODULE (`g.nodes` order, record order): its spec resolved like a GD.42
///    `require(..)` (the builder's `resolve_source`, then `<q>::index`); a
///    module holding no page reads one hop on, the unique module its import
///    statements resolve to that holds one (the NgModule form, `x.module`
///    importing `x-routing.module`), else `no_table`;
/// 3. each resolved mount flows the includer slot's prefixes joined with its
///    own prefix into the target's slot; a slot no resolved mount reaches is
///    a root holding `""`, so a repo without `loadChildren` keeps every page;
///    the fixpoint and its caps are CB.20's;
/// 4. each page re-keyed to `page:/<prefix>/<path>` once per prefix of its
///    slot (the first by rename, a page another file also minted absorbing
///    it as two same-path pages merge, each further prefix a clone); one no
///    root reaches keeps its own path;
/// 5. every NAVIGATES_TO ref whose link is a provisional
///    [`glia_code_domain::endpoint::nav_mount_link`] to `/<prefix><path>`: the
///    prefix its own page was mounted at when the link names that page's
///    slot, else the slot's first prefix, else none. No provisional link
///    reaches the link resolver (`crate::nav`).
///
/// Run between the TS surface and the stitch, after GD.42's
/// [`bind_ts_mounts`], so the HANDLED_BY and NAVIGATES_TO refs resolve from
/// the final pages.
pub(crate) fn bind_nav_mounts<R>(
    g: &mut RepoGraph,
    refs: &mut Vec<UnresolvedRef>,
    imports: &[ImportStmt],
    resolve_source: &R,
) -> NavMountStats
where
    R: Fn(&str, &str) -> Option<String>,
{
    let mut stats = NavMountStats::default();
    let provisionals = collect_nav_provisionals(g, refs);
    if provisionals.is_empty() {
        return stats;
    }
    let tables: BTreeSet<String> = provisionals.iter().map(|p| p.module.clone()).collect();
    stats.tables = tables.len();
    stats.routes = provisionals.len();

    // The mounts, resolved to flows between nav slots.
    let mut slot_modules: BTreeSet<String> = tables.clone();
    let mut includes: Vec<Flow> = Vec::new();
    let mut included: BTreeSet<String> = BTreeSet::new();
    {
        let no_facts = TsFacts::default();
        let resolver = TsResolver {
            g,
            imports,
            resolve_source,
            facts: &no_facts,
        };
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
                continue;
            }
            let (Some(m), Some(fs)) = (g.nav.qname_by_id.get(&n.id), g.nav.nav_facts.get(&n.id))
            else {
                continue;
            };
            for f in fs {
                let NavFact::IncludeMount {
                    target,
                    prefix,
                    via,
                    ..
                } = f
                else {
                    continue;
                };
                let legacy = if via.starts_with(VIA_NAV_LAZY_STRING) {
                    true
                } else if via.starts_with(VIA_NAV_LAZY) {
                    false
                } else {
                    continue;
                };
                stats.lazy += 1;
                slot_modules.insert(m.clone());
                let IncludeTarget::Spec(spec) = target else {
                    stats.dynamic += 1;
                    continue;
                };
                if legacy {
                    stats.string += 1;
                } else {
                    stats.import += 1;
                }
                let Some(module) = resolver.spec_module(m, spec) else {
                    stats.unresolved += 1;
                    continue;
                };
                let target = if tables.contains(&module) {
                    module
                } else if let Some(hop) = ngmodule_hop(&resolver, &module, &tables) {
                    stats.ngmodule_hop += 1;
                    hop
                } else {
                    stats.no_table += 1;
                    continue;
                };
                if &target != m {
                    included.insert(target.clone());
                }
                includes.push(Flow {
                    target: Slot::Module(target, NAV_VAR.to_string()),
                    mount: Mount::Module {
                        module: m.clone(),
                        var: NAV_VAR.to_string(),
                        suffix: prefix.clone(),
                    },
                });
            }
        }
    }

    // Roots first (by module), then the mounts in fact order.
    let mut flows: Vec<Flow> = slot_modules
        .iter()
        .filter(|m| !included.contains(*m))
        .map(|m| Flow {
            target: Slot::Module(m.clone(), NAV_VAR.to_string()),
            mount: Mount::Const(String::new()),
        })
        .collect();
    stats.roots = flows.len();
    flows.extend(includes);
    let (slots, capped) = fixpoint(&flows, &BTreeMap::new());
    stats.capped = capped;
    let prefixes_of = |module: &str| {
        slots
            .get(&Slot::Module(module.to_string(), NAV_VAR.to_string()))
            .filter(|s| !s.is_empty())
    };

    // Each provisional's finals: a path per prefix, sorted, the first
    // prefix kept for a path two prefixes reach.
    let mut plans: Vec<Vec<NavFinal>> = Vec::with_capacity(provisionals.len());
    for p in &provisionals {
        let mut paths: BTreeMap<String, String> = BTreeMap::new();
        match prefixes_of(&p.module) {
            Some(set) => {
                if included.contains(&p.module) {
                    stats.mounted += 1;
                }
                for prefix in set {
                    paths
                        .entry(nav_join(prefix, &p.path))
                        .or_insert_with(|| prefix.clone());
                }
            }
            None => {
                paths.insert(p.path.clone(), String::new());
            }
        }
        stats.copies += paths.len().saturating_sub(1);
        plans.push(
            paths
                .into_iter()
                .map(|(path, prefix)| NavFinal::new(g, path, prefix, p.owner.as_deref()))
                .collect(),
        );
    }

    // Rename each to its first final, then clone the rest.
    let renames: Vec<(NodeId, NodeId)> = provisionals
        .iter()
        .zip(&plans)
        .filter_map(|(p, finals)| finals.first().map(|f| (p.id, f.id)))
        .collect();
    let absorbed = super::rename_nodes(g, &mut [], refs, &renames);
    let remap: HashMap<NodeId, NodeId> = renames.iter().copied().collect();
    // Final page -> (its slot's module, the prefix it was mounted at): the
    // first provisional that reached it.
    let mut mounted_at: HashMap<NodeId, (String, String)> = HashMap::new();
    for (p, finals) in provisionals.iter().zip(&plans) {
        for f in finals {
            mounted_at
                .entry(f.id)
                .or_insert_with(|| (p.module.clone(), f.prefix.clone()));
        }
        if let Some(f) = finals.first()
            && !absorbed.contains(&p.id)
        {
            g.nav.qname_by_id.insert(f.id, f.qname.clone());
            g.nav.name_by_id.insert(f.id, f.name.clone());
        }
    }
    let mut at: HashMap<NodeId, usize> =
        g.nodes.iter().enumerate().map(|(i, n)| (n.id, i)).collect();
    for (p, finals) in provisionals.iter().zip(&plans) {
        for f in finals.iter().skip(1) {
            clone_nav_page(g, refs, &mut at, &remap, p, f);
        }
    }

    // The provisional redirect links.
    for r in refs.iter_mut().chain(g.unresolved_refs.iter_mut()) {
        if r.category != edge_category::NAVIGATES_TO {
            continue;
        }
        let CallQualifier::Bare(link) = &mut r.qualifier else {
            continue;
        };
        let Some((module, path)) = parse_nav_mount_link(link) else {
            continue;
        };
        let prefix = match mounted_at.get(&r.from) {
            Some((own, prefix)) if own == module => prefix.clone(),
            _ => prefixes_of(module)
                .and_then(|set| set.iter().next())
                .cloned()
                .unwrap_or_default(),
        };
        *link = nav_join(&prefix, path);
        stats.links_rewritten += 1;
    }
    debug_assert!(
        g.nodes.iter().all(|n| g
            .nav
            .qname_by_id
            .get(&n.id)
            .is_none_or(|q| parse_nav_mount_route_qname(split_owner(q).0).is_none())),
        "a provisional nav page survived the nav mount pass"
    );
    stats
}

/// The unique module `module`'s own import statements resolve to that holds
/// a provisional nav page (the NgModule form: `items.module` imports
/// `items-routing.module`, which declares the table); `None` for none or two.
fn ngmodule_hop<R>(
    resolver: &TsResolver<'_, R>,
    module: &str,
    tables: &BTreeSet<String>,
) -> Option<String>
where
    R: Fn(&str, &str) -> Option<String>,
{
    let mut found: BTreeSet<String> = BTreeSet::new();
    for s in resolver.imports.iter().filter(|s| s.from_module == module) {
        let spec = match &s.target {
            ImportTarget::Module { path, .. } => path,
            ImportTarget::Symbol { module: m, .. } => m,
        };
        if let Some(t) = resolver.spec_module(module, spec)
            && t != module
            && tables.contains(&t)
        {
            found.insert(t);
        }
    }
    let mut picks = found.into_iter();
    match (picks.next(), picks.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

/// Every ROUTE whose qname (owner stripped) is a provisional nav page, in
/// `g.nodes` order, with its cells, incident edges and pending refs.
fn collect_nav_provisionals(g: &RepoGraph, refs: &[UnresolvedRef]) -> Vec<NavProvisional> {
    let mut out: Vec<NavProvisional> = Vec::new();
    for n in &g.nodes {
        if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ROUTE) {
            continue;
        }
        let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
            continue;
        };
        let (bare, owner) = split_owner(qname);
        let Some((module, path)) = parse_nav_mount_route_qname(bare) else {
            continue;
        };
        out.push(NavProvisional {
            id: n.id,
            module: module.to_string(),
            path: path.to_string(),
            owner: owner.map(str::to_string),
            confidence: n.confidence,
            cells: n.cells.clone(),
            edges: Vec::new(),
            refs: Vec::new(),
        });
    }
    if out.is_empty() {
        return out;
    }
    let index: HashMap<NodeId, usize> = out.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
    for e in &g.edges {
        let from = index.get(&e.from).copied();
        let to = index.get(&e.to).copied();
        if let Some(i) = from {
            out[i].edges.push(e.clone());
        }
        if let Some(i) = to
            && Some(i) != from
        {
            out[i].edges.push(e.clone());
        }
    }
    for r in refs {
        if let Some(&i) = index.get(&r.from) {
            out[i].refs.push(r.clone());
        }
    }
    out
}

/// Add final `f` of provisional `p` as its own page (CB.20's clone rule,
/// `go_mounts`): `p`'s cells and a nav record (no parent), a copy of each of
/// `p`'s edges with `f` for `p` (the other end through `remap`), a copy of
/// each of its refs from `f`. An id already in the graph takes the cells and
/// keeps its nav record.
fn clone_nav_page(
    g: &mut RepoGraph,
    refs: &mut Vec<UnresolvedRef>,
    at: &mut HashMap<NodeId, usize>,
    remap: &HashMap<NodeId, NodeId>,
    p: &NavProvisional,
    f: &NavFinal,
) {
    match at.get(&f.id) {
        Some(&i) => g.nodes[i].cells.extend(p.cells.iter().cloned()),
        None => {
            at.insert(f.id, g.nodes.len());
            g.nodes.push(glia_core::Node {
                id: f.id,
                repo: g.repo,
                confidence: p.confidence,
                cells: p.cells.clone(),
            });
            g.nav
                .record(f.id, &f.name, &f.qname, node_kind::ROUTE, None);
        }
    }
    let end = |id: NodeId| {
        if id == p.id {
            f.id
        } else {
            remap.get(&id).copied().unwrap_or(id)
        }
    };
    for e in &p.edges {
        let mut copy = e.clone();
        copy.from = end(e.from);
        copy.to = end(e.to);
        g.edges.push(copy);
    }
    for r in &p.refs {
        let mut copy = r.clone();
        copy.from = f.id;
        refs.push(copy);
    }
}

#[cfg(test)]
mod tests {
    //! Python graphs built from parsed urlconfs (the Python parser records
    //! the provisional ROUTEs and the mount facts), then `build_python`, which
    //! runs the pass.

    use glia_core::RepoId;

    use super::*;
    use crate::build::build_python;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://route_mounts")
    }

    /// `a/b/c.py` -> `a::b::c`, `a/__init__.py` -> `a::__init__`.
    fn parse(rel: &str, src: &str) -> glia_code_domain::FileParse {
        let q = rel.strip_suffix(".py").unwrap_or(rel).replace('/', "::");
        glia_parser_python::parse_file(src, rel, &q, repo()).expect("parse")
    }

    fn build(files: &[(&str, &str)]) -> RepoGraph {
        build_python(repo(), files.iter().map(|(r, s)| parse(r, s)).collect()).expect("build")
    }

    /// The pass alone over the merged parses, as `build_python` runs it.
    fn stats(files: &[(&str, &str)]) -> RouteMountStats {
        let parses: Vec<_> = files.iter().map(|(r, s)| parse(r, s)).collect();
        let (mut g, imports, _, mut refs) = crate::build::merge_parses(repo(), parses);
        crate::build::build_symbol_table(&mut g);
        bind_module_mounts(&mut g, &mut refs, &imports)
    }

    /// Every ROUTE qname of `g`, sorted.
    fn routes(g: &RepoGraph) -> Vec<String> {
        let mut out: Vec<String> = g
            .nodes
            .iter()
            .filter(|n| g.nav.kind_by_id.get(&n.id) == Some(&node_kind::ROUTE))
            .filter_map(|n| g.nav.qname_by_id.get(&n.id).cloned())
            .collect();
        out.sort();
        out
    }

    /// The `file:line` (0-based) of each copy of route `qname`'s POSITION.
    fn positions(g: &RepoGraph, qname: &str) -> Vec<String> {
        let id = NodeId::from_parts(
            glia_code_domain::GRAPH_TYPE,
            repo(),
            node_kind::ROUTE,
            qname,
        );
        g.nodes
            .iter()
            .filter(|n| n.id == id)
            .filter_map(|n| glia_code_domain::evidence::locate(&n.cells))
            .map(|(file, line)| format!("{file}:{}", line.unwrap_or(u32::MAX)))
            .collect()
    }

    const ROOT: &str = "from django.urls import include, path\n\nurlpatterns = [\n    path(\"api/\", include(\"app.urls\")),\n    path(\"auth/\", include(\"app.auth_urls\")),\n]\n";
    const APP: &str = "from django.urls import include, path\n\nfrom .issue import urlpatterns as issue_urls\n\nurlpatterns = [\n    *issue_urls,\n    path(\"v2/\", include(\"app.urls.nested\")),\n]\n";
    const ISSUE: &str = "from django.urls import path\n\nurlpatterns = [\n    path(\"workspaces/<str:slug>/issues/\", issue_list),\n]\n\n\ndef issue_list(request, slug):\n    return []\n";
    const NESTED: &str =
        "from django.urls import path\n\nurlpatterns = [\n    path(\"things/\", things),\n]\n";
    const AUTH: &str =
        "from django.urls import path\n\nurlpatterns = [\n    path(\"login/\", login),\n]\n";

    fn probe() -> Vec<(&'static str, &'static str)> {
        vec![
            ("proj/urls.py", ROOT),
            ("app/urls/__init__.py", APP),
            ("app/urls/issue.py", ISSUE),
            ("app/urls/nested.py", NESTED),
            ("app/auth_urls.py", AUTH),
        ]
    }

    /// The probe tree: a nested include, a spread import, every route at its
    /// full path, located at its call, and no `ANY api/`.
    #[test]
    fn nested_includes_and_spreads_mount_every_route() {
        let g = build(&probe());
        assert_eq!(
            routes(&g),
            [
                "ANY /api/v2/things/",
                "ANY /api/workspaces/<str:slug>/issues/",
                "ANY /auth/login/",
            ]
        );
        assert_eq!(
            positions(&g, "ANY /api/workspaces/<str:slug>/issues/"),
            ["app/urls/issue.py:3"]
        );
        let s = stats(&probe());
        assert_eq!(
            s.marker("python").as_deref(),
            Some(
                "[route-mounts] lang=python includes=3 spreads=1 dynamic=0 \
                 via(flask=0 fastapi=0 django=4) unresolved=0 \
                 modules=5 routes=3 mounted=3 copies=0 roots=1 capped=0"
            )
        );
    }

    /// A body-only re-stitch never re-keys a mount: a urlconf whose ROUTEs
    /// the pass renamed is refused (its fresh parse mints the provisional
    /// ids again), so the watch rebuilds the group instead.
    #[test]
    fn a_rebody_of_a_route_module_is_refused() {
        let mut g = build(&probe());
        // The view's body only: the declarations stay as they were.
        let edited = ISSUE.replace("return []", "return [slug]");
        let got = crate::build::rebuild_python_bodies(
            &mut g,
            vec![crate::build::BodyEdit::new(parse(
                "app/urls/issue.py",
                &edited,
            ))],
        );
        assert!(got.is_err(), "{got:?}");
        assert_eq!(
            routes(&g),
            [
                "ANY /api/v2/things/",
                "ANY /api/workspaces/<str:slug>/issues/",
                "ANY /auth/login/",
            ]
        );
    }

    /// One module included under two prefixes is two ROUTEs at one POSITION.
    #[test]
    fn two_includes_of_one_module_are_two_routes() {
        let root = "from django.urls import include, path\n\nurlpatterns = [\n    path(\"api/\", include(\"app.auth_urls\")),\n    path(\"legacy/\", include(\"app.auth_urls\")),\n]\n";
        let files = [("proj/urls.py", root), ("app/auth_urls.py", AUTH)];
        let g = build(&files);
        assert_eq!(routes(&g), ["ANY /api/login/", "ANY /legacy/login/"]);
        assert_eq!(positions(&g, "ANY /legacy/login/"), ["app/auth_urls.py:3"]);
        assert_eq!(stats(&files).copies, 1);
    }

    /// A cycle stops at the cap; an include of a module outside the graph is
    /// unresolved; a dynamic include mounts nothing; a urlconf no include
    /// reaches is a root at its local path.
    #[test]
    fn cycles_unresolved_and_dynamic_includes() {
        let a = "from django.urls import include, path\n\nurlpatterns = [\n    path(\"a/\", view_a),\n    path(\"b/\", include(\"loop.b\")),\n]\n";
        let b = "from django.urls import include, path\n\nurlpatterns = [\n    path(\"back/\", include(\"loop.a\")),\n]\n";
        let root = "from django.urls import include, path\n\nurlpatterns = [\n    path(\"x/\", include(\"loop.a\")),\n    path(\"admin/\", include(\"django.contrib.admin.urls\")),\n    path(\"r/\", include(router.urls)),\n    path(\"health/\", health),\n]\n";
        let files = [("proj/urls.py", root), ("loop/a.py", a), ("loop/b.py", b)];
        let s = stats(&files);
        assert_eq!(
            (s.includes, s.unresolved, s.dynamic, s.roots, s.capped),
            (4, 1, 1, 1, 2)
        );
        let g = build(&files);
        let rs = routes(&g);
        assert!(rs.contains(&"ANY /health/".to_string()), "{rs:?}");
        assert!(rs.contains(&"ANY /x/a/".to_string()), "{rs:?}");
        assert!(rs.contains(&"ANY /x/b/back/a/".to_string()), "{rs:?}");
        // The loop's prefixes grow one per fixpoint round until the cap.
        let copies = rs.iter().filter(|r| r.ends_with("/a/")).count();
        assert!(copies > 2 && copies <= go_mounts::MAX_PREFIXES, "{copies}");
        assert!(
            !rs.iter()
                .any(|r| r.contains("<mount:") || r.contains("admin"))
        );
    }

    /// Two candidate urlconfs for one dotted tail: the one in the includer's
    /// own project wins; with no includer to share a prefix, none does.
    #[test]
    fn a_dotted_tail_prefers_the_includers_project() {
        let root = "from django.urls import include, path\n\nurlpatterns = [\n    path(\"api/\", include(\"app.auth_urls\")),\n]\n";
        let files = [
            ("svc_a/proj/urls.py", root),
            ("svc_a/app/auth_urls.py", AUTH),
            ("svc_b/app/auth_urls.py", AUTH),
        ];
        let g = build(&files);
        // svc_b's urlconf is a root of its own: `/login/` at its local path.
        assert_eq!(routes(&g), ["ANY /api/login/", "ANY /login/"]);
        assert_eq!(
            positions(&g, "ANY /api/login/"),
            ["svc_a/app/auth_urls.py:3"]
        );
    }

    /// GD.41: Flask. A blueprint with no `url_prefix` mounted cross-file at
    /// `/svc` by imported name; one with its own `/orders` registered by
    /// module attribute with none keeps its own; a register's `url_prefix`
    /// REPLACES a blueprint's own; a nested blueprint composes under its
    /// parent's.
    #[test]
    fn flask_register_blueprint_mounts_across_files() {
        let items = "from flask import Blueprint\n\nbp = Blueprint(\"items\", __name__)\n\n\n@bp.route(\"/items\")\ndef list_items():\n    return []\n";
        let orders = "from flask import Blueprint\n\norders = Blueprint(\"orders\", __name__, url_prefix=\"/orders\")\n\n\n@orders.get(\"/recent\")\ndef recent():\n    return []\n";
        let legacy = "from flask import Blueprint\n\nold = Blueprint(\"old\", __name__, url_prefix=\"/ignored\")\nchild = Blueprint(\"child\", __name__, url_prefix=\"/child\")\nold.register_blueprint(child)\n\n\n@old.get(\"/a\")\ndef a():\n    return []\n\n\n@child.get(\"/b\")\ndef b():\n    return []\n";
        let app = "from flask import Flask\n\nfrom svc.items import bp\nfrom svc import orders, legacy\n\napp = Flask(__name__)\napp.register_blueprint(bp, url_prefix=\"/svc\")\napp.register_blueprint(orders.orders)\napp.register_blueprint(legacy.old, url_prefix=\"/v1\")\n\n\n@app.route(\"/health\")\ndef health():\n    return \"ok\"\n";
        let files = [
            ("app.py", app),
            ("svc/items.py", items),
            ("svc/orders.py", orders),
            ("svc/legacy.py", legacy),
        ];
        let g = build(&files);
        assert_eq!(
            routes(&g),
            [
                "GET /health",
                "GET /orders/recent",
                "GET /svc/items",
                "GET /v1/a",
                "GET /v1/child/b",
            ]
        );
        let s = stats(&files);
        assert_eq!(
            (s.via_flask, s.via_fastapi, s.unresolved, s.mounted),
            (4, 0, 0, 4)
        );
    }

    /// GD.41: FastAPI. A two-level include chain across files (module
    /// attribute, then an aliased imported name) composes the include
    /// prefixes, the including router's own and the router subclass's own;
    /// an unresolved include leaves its router at its own path.
    #[test]
    fn fastapi_include_router_chain_composes() {
        let base = "from fastapi import APIRouter\n\n\nclass ServiceRouter(APIRouter):\n    pass\n";
        let users = "from fastapi import APIRouter\n\nrouter = APIRouter()\n\n\n@router.get(\"/users\")\ndef list_users():\n    return []\n";
        let flows = "from core.base import ServiceRouter\n\nrouter: ServiceRouter = ServiceRouter(prefix=\"/flows\")\n\n\n@router.post(\"/filter\")\ndef filter_flows():\n    return []\n";
        let init = "from fastapi import APIRouter\n\nfrom . import flows, users\n\nrouter = APIRouter(prefix=\"/core\")\nrouter.include_router(users.router)\nrouter.include_router(flows.router, prefix=\"/v1\")\n";
        let main = "from fastapi import FastAPI\n\nfrom routers import router as core_router\nfrom thirdparty import admin\n\napp = FastAPI()\napp.include_router(core_router, prefix=\"/svc\")\napp.include_router(admin.router)\n";
        let files = [
            ("main.py", main),
            ("core/base.py", base),
            ("routers/__init__.py", init),
            ("routers/users.py", users),
            ("routers/flows.py", flows),
        ];
        let g = build(&files);
        assert_eq!(
            routes(&g),
            ["GET /svc/core/users", "POST /svc/core/v1/flows/filter"]
        );
        let s = stats(&files);
        assert_eq!(
            s.marker("python").as_deref(),
            Some(
                "[route-mounts] lang=python includes=4 spreads=0 dynamic=0 \
                 via(flask=0 fastapi=4 django=0) unresolved=1 \
                 modules=4 routes=2 mounted=2 copies=0 roots=1 capped=0"
            )
        );
        // Unmounted: the same router files alone are roots at their own path.
        let alone = build(&files[1..]);
        assert_eq!(
            routes(&alone),
            ["GET /core/users", "POST /core/v1/flows/filter"]
        );
    }

    /// GD.41: mealie's shape. Controllers on `UserAPIRouter(prefix=..)` (an
    /// imported APIRouter subclass) inside a class, a package router with
    /// its own prefix including them, `routes/__init__.py`'s
    /// `APIRouter(prefix="/api")` including the package, and the app
    /// including that inside a factory function; `@router.post("")` is the
    /// router's own path.
    #[test]
    fn nested_package_routers_compose_inside_a_factory() {
        let ctrl = "from app.routes._base.routers import UserAPIRouter\n\nrouter = UserAPIRouter(prefix=\"/households/mealplans/rules\")\n\n\nclass Controller:\n    @router.get(\"/{item_id}\")\n    def get_one(self, item_id: str):\n        return item_id\n\n    @router.post(\"\", status_code=201)\n    def create_one(self):\n        return 1\n";
        let households = "from fastapi import APIRouter\n\nfrom . import controller_rules\n\nrouter = APIRouter()\nrouter.include_router(controller_rules.router, tags=[\"x\"])\n";
        let routes_init = "from fastapi import APIRouter\n\nfrom . import households\n\nrouter = APIRouter(prefix=\"/api\")\nrouter.include_router(households.router)\n";
        let app = "from fastapi import FastAPI\n\nfrom app.routes import router\n\napp = FastAPI()\n\n\ndef api_routers():\n    app.include_router(router)\n";
        let files = [
            ("app/app.py", app),
            ("app/routes/__init__.py", routes_init),
            ("app/routes/households/__init__.py", households),
            ("app/routes/households/controller_rules.py", ctrl),
        ];
        let g = build(&files);
        assert_eq!(
            routes(&g),
            [
                "GET /api/households/mealplans/rules/{item_id}",
                "POST /api/households/mealplans/rules",
            ]
        );
    }
    // ---- GD.42: TypeScript / JavaScript ----------------------------------

    /// The MODULE qname of a TS file path: extension dropped, `/` -> `::`.
    fn ts_qname(path: &str) -> String {
        path.rsplit_once('.')
            .map_or(path, |(stem, _)| stem)
            .replace('/', "::")
    }

    /// A TS / JS file parsed, then its backend routes extracted onto it as
    /// the engine does (`engine/src/extract.rs`): nodes, refs, names and the
    /// mount facts on the MODULE.
    fn ts_parse(rel: &str, src: &str) -> glia_code_domain::FileParse {
        let q = ts_qname(rel);
        let mut fp = glia_parser_typescript::parse_file(src, rel, &q, repo()).expect("parse");
        let module =
            NodeId::from_parts(glia_code_domain::GRAPH_TYPE, repo(), node_kind::MODULE, &q);
        let routes = glia_code_extractors::ts_routes::extract_ts_backend_routes(
            src,
            rel,
            module,
            &q,
            repo(),
        );
        fp.nodes.extend(routes.nodes);
        fp.refs.extend(routes.refs);
        fp.nav.name_by_id.extend(routes.nav.name_by_id);
        fp.nav.qname_by_id.extend(routes.nav.qname_by_id);
        fp.nav.kind_by_id.extend(routes.nav.kind_by_id);
        fp.nav.parent_of.extend(routes.nav.parent_of);
        for (k, v) in routes.nav.children_of {
            fp.nav.children_of.entry(k).or_default().extend(v);
        }
        for f in routes.mount_facts {
            fp.nav.record_fact(module, f);
        }
        fp
    }

    /// A relative specifier resolved against the importer's qname (`.js` /
    /// `.ts` dropped); a bare one is external.
    fn ts_source(from: &str, spec: &str) -> Option<String> {
        if !spec.starts_with('.') {
            return None;
        }
        let mut segs: Vec<&str> = from.split("::").collect();
        segs.pop();
        for part in spec.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    segs.pop();
                }
                p => segs.push(
                    p.strip_suffix(".ts")
                        .or_else(|| p.strip_suffix(".js"))
                        .unwrap_or(p),
                ),
            }
        }
        Some(segs.join("::"))
    }

    fn ts_build(files: &[(&str, &str)]) -> RepoGraph {
        crate::build::build_typescript_family(
            repo(),
            files.iter().map(|(r, s)| ts_parse(r, s)).collect(),
            ts_source,
        )
        .expect("build")
    }

    /// The pass alone over the TS surface, as `build_ts_like` runs it.
    fn ts_stats(files: &[(&str, &str)]) -> TsMountStats {
        let parses: Vec<_> = files.iter().map(|(r, s)| ts_parse(r, s)).collect();
        let (mut g, mut body, stmts) = crate::build::ts_surface(repo(), parses, &ts_source);
        bind_ts_mounts(&mut g, &mut body.refs, &stmts, &ts_source)
    }

    const TS_USERS: &str = "const express = require('express');\nconst router = express.Router();\nrouter.get('/list', listUsers);\n\nfunction listUsers(req, res) {\n  res.json([]);\n}\n\nmodule.exports = router;\n";
    const TS_ITEMS: &str = "import { Router } from 'express';\nconst router = Router();\nrouter.get('/all', (req, res) => res.json([]));\n\nexport default router;\n";
    const TS_APP: &str = "import express from 'express';\nimport items from './routes/items';\nimport helmet from 'helmet';\nimport { auth } from './auth';\n\nconst app = express();\nconst local = express.Router();\nlocal.get('/ping', (req, res) => res.send('ok'));\napp.use('/api/users', require('./routes/users'));\napp.use('/api/items', items);\napp.use('/api/local', local);\napp.use('/api/legacy', local);\napp.use('/api', auth);\napp.use(helmet);\napp.get('/version', (req, res) => res.send('1'));\n";
    const TS_AUTH: &str = "export function auth(req, res, next) {\n  next();\n}\n";

    /// The probe tree: an inline `require(..)`, a default import, a
    /// same-file router mounted twice; a middleware mounts nothing, an
    /// external import is unresolved, the app's own route keeps its path.
    #[test]
    fn express_use_mounts_every_router() {
        let files = [
            ("app.ts", TS_APP),
            ("auth.ts", TS_AUTH),
            ("routes/users.js", TS_USERS),
            ("routes/items.ts", TS_ITEMS),
        ];
        let g = ts_build(&files);
        assert_eq!(
            routes(&g),
            [
                "GET /api/items/all",
                "GET /api/legacy/ping",
                "GET /api/local/ping",
                "GET /api/users/list",
                "GET /version",
            ]
        );
        assert_eq!(positions(&g, "GET /api/users/list"), ["routes/users.js:2"]);
        assert_eq!(
            ts_stats(&files).marker().as_deref(),
            Some(
                "[route-mounts] lang=typescript includes=4 dynamic=1 \
                 via(express=6 hono=0 koa=0 nest_global=0 nest_router=0) \
                 nest_modules=0 unresolved=1 modules=3 routes=4 mounted=3 \
                 copies=1 roots=1 capped=0 excluded=0"
            )
        );
        // The named handler follows the re-keyed ROUTE.
        let route = NodeId::from_parts(
            glia_code_domain::GRAPH_TYPE,
            repo(),
            node_kind::ROUTE,
            "GET /api/users/list",
        );
        assert!(
            g.edges
                .iter()
                .any(|e| e.from == route
                    && e.category == glia_code_domain::edge_category::HANDLED_BY),
            "no HANDLED_BY from the mounted route"
        );
        // Unmounted, each router is a root at its own path.
        let alone = ts_build(&files[2..]);
        assert_eq!(routes(&alone), ["GET /all", "GET /list"]);
    }

    /// documenso's Hono shape: chained sub-apps mounted two levels deep by
    /// imported name under a root with a non-literal `basePath`; a literal
    /// `basePath` and a Koa `prefix` are a router's own.
    #[test]
    fn hono_and_koa_mounts_compose() {
        let files = "import { Hono } from 'hono';\n\nexport const filesRoute = new Hono()\n  .post('/upload-pdf', async (c) => {\n    return c.json({});\n  })\n  .get('/item/:id', (c) => c.json({}));\n";
        let auth = "import { Hono } from 'hono';\n\nconst session = new Hono().get('/session', (c) => c.json({}));\n\nexport const auth = new Hono()\n  .route('/', session)\n  .post('/sign-out', (c) => c.json({}));\n";
        let router = "import { Hono } from 'hono';\nimport { auth } from './api/auth';\nimport { filesRoute } from './api/files';\n\nconst app = new Hono().basePath(process.env.BASE || '/');\napp.route('/api/auth', auth);\napp.route('/api/files', filesRoute);\nconst v2 = new Hono().basePath('/v2');\nv2.get('/x', (c) => c.json({}));\napp.route('/api', v2);\n";
        let koa = "const Router = require('@koa/router');\nconst sub = new Router();\nsub.get('/b', h);\nconst api = new Router({ prefix: '/v1' });\napi.use('/sub', sub.routes(), sub.allowedMethods());\n";
        let g = ts_build(&[
            ("router.ts", router),
            ("api/auth.ts", auth),
            ("api/files.ts", files),
            ("koa.js", koa),
        ]);
        assert_eq!(
            routes(&g),
            [
                "GET /api/auth/session",
                "GET /api/files/item/:id",
                "GET /api/v2/x",
                "GET /v1/sub/b",
                "POST /api/auth/sign-out",
                "POST /api/files/upload-pdf",
            ]
        );
    }

    const NEST_CTRL: &str = "import { Controller, Get, Post } from '@nestjs/common';\n\n@Controller('users')\nexport class UsersController {\n  @Get('all')\n  all() {\n    return [];\n  }\n\n  @Post()\n  create() {\n    return {};\n  }\n}\n";
    const NEST_HEALTH: &str = "import { Controller, Get } from '@nestjs/common';\n\n@Controller()\nexport class HealthController {\n  @Get('health')\n  health() {\n    return 1;\n  }\n\n  @Get('.well-known/app')\n  wellKnown() {\n    return 1;\n  }\n\n  @Get('version')\n  version() {\n    return 1;\n  }\n}\n";
    const NEST_ADMIN: &str = "import { Module } from '@nestjs/common';\nimport { UsersController } from './users.controller';\n\n@Module({\n  controllers: [UsersController],\n})\nexport class AdminModule {}\n";
    const NEST_APP: &str = "import { Module } from '@nestjs/common';\nimport { RouterModule } from '@nestjs/core';\nimport { AdminModule } from './admin/admin.module';\n\n@Module({\n  imports: [RouterModule.register([{ path: 'backoffice', module: AdminModule }])],\n})\nexport class AppModule {}\n";
    const NEST_CONSTS: &str = "export const excludePaths = ['/.well-known/app'];\n";
    const NEST_MAIN: &str = "import { NestFactory } from '@nestjs/core';\nimport { AppModule } from './app.module';\nimport { excludePaths } from './constants';\n\nasync function bootstrap() {\n  const app = await NestFactory.create(AppModule);\n  app.setGlobalPrefix('svc', { exclude: excludePaths });\n  app.setGlobalPrefix('svc', { exclude: [{ path: 'health', method: RequestMethod.GET }] });\n}\n";

    /// NestJS: the global prefix on every controller route, a RouterModule
    /// path below it for its module's controllers, and the excludes (an
    /// imported path list, a `{ path, method }`) keep their own path.
    #[test]
    fn nest_global_prefix_and_router_module() {
        let files = [
            ("src/main.ts", NEST_MAIN),
            ("src/constants.ts", NEST_CONSTS),
            ("src/app.module.ts", NEST_APP),
            ("src/admin/admin.module.ts", NEST_ADMIN),
            ("src/admin/users.controller.ts", NEST_CTRL),
            ("src/health.controller.ts", NEST_HEALTH),
        ];
        let g = ts_build(&files);
        assert_eq!(
            routes(&g),
            [
                "GET /.well-known/app",
                "GET /health",
                "GET /svc/backoffice/users/all",
                "GET /svc/version",
                "POST /svc/backoffice/users",
            ]
        );
        let s = ts_stats(&files);
        assert_eq!(
            (
                s.nest_global,
                s.nest_router,
                s.nest_modules,
                s.excluded,
                s.mounted
            ),
            (2, 1, 1, 2, 3)
        );
        // The decorated handler follows the re-keyed ROUTE.
        let route = NodeId::from_parts(
            glia_code_domain::GRAPH_TYPE,
            repo(),
            node_kind::ROUTE,
            "GET /svc/backoffice/users/all",
        );
        assert!(
            g.edges
                .iter()
                .any(|e| e.from == route
                    && e.category == glia_code_domain::edge_category::HANDLED_BY)
        );
        // With no global prefix nor RouterModule, Nest routes are untouched.
        let plain = ts_build(&files[3..]);
        assert_eq!(
            routes(&plain),
            [
                "GET /.well-known/app",
                "GET /health",
                "GET /users/all",
                "GET /version",
                "POST /users",
            ]
        );
    }

    /// Two Nest apps in one repo: each controller takes the global prefix of
    /// the `setGlobalPrefix` nearest it.
    #[test]
    fn nest_global_prefix_is_per_app() {
        let main =
            |p: &str| format!("async function boot() {{\n  app.setGlobalPrefix('{p}');\n}}\n");
        let (a, b) = (main("a"), main("b"));
        let g = ts_build(&[
            ("apps/one/src/main.ts", &a),
            ("apps/one/src/users.controller.ts", NEST_CTRL),
            ("apps/two/src/main.ts", &b),
            ("apps/two/src/health.controller.ts", NEST_HEALTH),
        ]);
        assert_eq!(
            routes(&g),
            [
                "GET /a/users/all",
                "GET /b/.well-known/app",
                "GET /b/health",
                "GET /b/version",
                "POST /a/users",
            ]
        );
    }

    /// A body-only re-stitch of a mounted router file is refused (its fresh
    /// parse mints the provisional ids again), so the watch rebuilds the
    /// group instead.
    #[test]
    fn a_rebody_of_a_mounted_ts_router_is_refused() {
        let files = [
            ("app.ts", TS_APP),
            ("auth.ts", TS_AUTH),
            ("routes/users.js", TS_USERS),
            ("routes/items.ts", TS_ITEMS),
        ];
        let mut g = ts_build(&files);
        let edited = TS_USERS.replace("res.json([])", "res.json([1])");
        let got = crate::build::rebuild_ts_bodies(
            &mut g,
            vec![crate::build::BodyEdit::new(ts_parse(
                "routes/users.js",
                &edited,
            ))],
            ts_source,
        );
        assert!(got.is_err(), "{got:?}");
        assert!(routes(&g).contains(&"GET /api/users/list".to_string()));
    }

    // ---- HQ.5: client-router lazy child tables ---------------------------

    /// A TS-family file parsed, then its route tables and link sites
    /// extracted onto it as the engine does: the pages on the MODULE's
    /// provisional nav slot (`final_pages` mints them on their final qnames,
    /// the BASE shape) and each `loadChildren` as a mount fact.
    fn nav_parse_with(rel: &str, src: &str, final_pages: bool) -> glia_code_domain::FileParse {
        let q = ts_qname(rel);
        let mut fp = glia_parser_typescript::parse_file(src, rel, &q, repo()).expect("parse");
        let module =
            NodeId::from_parts(glia_code_domain::GRAPH_TYPE, repo(), node_kind::MODULE, &q);
        let slot = if final_pages { "" } else { q.as_str() };
        let tables =
            glia_code_extractors::nav_routes::extract_route_tables(src, module, slot, repo());
        fp.nodes.extend(tables.nodes);
        fp.refs.extend(tables.refs);
        fp.nav.name_by_id.extend(tables.nav.name_by_id);
        fp.nav.qname_by_id.extend(tables.nav.qname_by_id);
        fp.nav.kind_by_id.extend(tables.nav.kind_by_id);
        for f in tables.mounts {
            fp.nav.record_fact(module, f);
        }
        let links = glia_code_extractors::nav_links::extract_nav_links(src, "typescript", module);
        fp.refs.extend(links.refs);
        fp
    }

    fn nav_build_with(files: &[(&str, &str)], final_pages: bool) -> RepoGraph {
        crate::build::build_typescript_family(
            repo(),
            files
                .iter()
                .map(|(r, s)| nav_parse_with(r, s, final_pages))
                .collect(),
            ts_source,
        )
        .expect("build")
    }

    fn nav_build(files: &[(&str, &str)]) -> RepoGraph {
        nav_build_with(files, false)
    }

    /// The pass alone over the TS surface, as `build_ts_like` runs it.
    fn nav_stats(files: &[(&str, &str)]) -> NavMountStats {
        let parses: Vec<_> = files
            .iter()
            .map(|(r, s)| nav_parse_with(r, s, false))
            .collect();
        let (mut g, mut body, stmts) = crate::build::ts_surface(repo(), parses, &ts_source);
        bind_nav_mounts(&mut g, &mut body.refs, &stmts, &ts_source)
    }

    fn qname_of(g: &RepoGraph, id: NodeId) -> String {
        g.nav.qname_by_id.get(&id).cloned().unwrap_or_default()
    }

    /// `(from qname, to qname, rule)` of every edge of `category`, sorted.
    fn nav_edges(
        g: &RepoGraph,
        category: glia_core::EdgeCategoryId,
    ) -> Vec<(String, String, String)> {
        let mut out: Vec<(String, String, String)> = g
            .edges
            .iter()
            .filter(|e| e.category == category)
            .map(|e| {
                let rule = e
                    .cells
                    .iter()
                    .find_map(|c| match &c.payload {
                        glia_core::CellPayload::Json(j)
                            if c.kind == glia_code_domain::cell_type::EVIDENCE =>
                        {
                            j.split("\"rule\":\"")
                                .nth(1)
                                .and_then(|r| r.split('"').next())
                                .map(str::to_string)
                        }
                        _ => None,
                    })
                    .unwrap_or_default();
                (qname_of(g, e.from), qname_of(g, e.to), rule)
            })
            .collect();
        out.sort();
        out
    }

    /// The NAVIGATES_TO links left unresolved (the dead links), sorted.
    fn dead_links(g: &RepoGraph) -> Vec<String> {
        let mut out: Vec<String> = g
            .unresolved_refs
            .iter()
            .filter(|r| r.category == glia_code_domain::edge_category::NAVIGATES_TO)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(l) => Some(l.clone()),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    // The fixture nav-angular-lazy-children (squidex's public shape), read
    // from bench/substrate-gap so the unit test and the graded key share it.
    const NAV_APP: &str = include_str!(
        "../../../bench/substrate-gap/fixtures/nav-angular-lazy-children/src/app/app.routes.ts"
    );
    const NAV_PAGES: &str = include_str!(
        "../../../bench/substrate-gap/fixtures/nav-angular-lazy-children/src/app/pages.ts"
    );
    const NAV_ITEMS: &str = include_str!(
        "../../../bench/substrate-gap/fixtures/nav-angular-lazy-children/src/app/features/items/routes.ts"
    );
    const NAV_ITEMS_PAGE: &str = include_str!(
        "../../../bench/substrate-gap/fixtures/nav-angular-lazy-children/src/app/features/items/items-page.component.ts"
    );
    const NAV_ITEM_PAGE: &str = include_str!(
        "../../../bench/substrate-gap/fixtures/nav-angular-lazy-children/src/app/features/items/item-page.component.ts"
    );
    const NAV_GUARD: &str = include_str!(
        "../../../bench/substrate-gap/fixtures/nav-angular-lazy-children/src/app/shared/item-must-exist.guard.ts"
    );

    fn nav_fixture() -> Vec<(&'static str, &'static str)> {
        vec![
            ("src/app/app.routes.ts", NAV_APP),
            ("src/app/pages.ts", NAV_PAGES),
            ("src/app/features/items/routes.ts", NAV_ITEMS),
            (
                "src/app/features/items/items-page.component.ts",
                NAV_ITEMS_PAGE,
            ),
            (
                "src/app/features/items/item-page.component.ts",
                NAV_ITEM_PAGE,
            ),
            ("src/app/shared/item-must-exist.guard.ts", NAV_GUARD),
        ]
    }

    /// The fixture, then a two-level chain, a relative redirect in a lazy
    /// table and an unresolved spec: every lazy page composes under its
    /// parent's path, the literal `/404` is a dead link, the true deep link
    /// binds.
    #[test]
    fn nav_lazy_tables_compose() {
        let g = nav_build(&nav_fixture());
        assert_eq!(
            routes(&g),
            [
                "page:/",
                "page:/**",
                "page:/app",
                "page:/app/items",
                "page:/app/items/:itemId"
            ]
        );
        let handled = nav_edges(&g, glia_code_domain::edge_category::HANDLED_BY);
        let h = |route: &str, to: &str| handled.iter().any(|(f, t, _)| f == route && t == to);
        assert!(h("page:/", "src::app::pages::HomeComponent"), "{handled:?}");
        assert!(
            h(
                "page:/app/items",
                "src::app::features::items::items-page.component::ItemsPageComponent"
            ),
            "{handled:?}"
        );
        assert!(
            h(
                "page:/app/items/:itemId",
                "src::app::features::items::item-page.component::ItemPageComponent"
            ),
            "{handled:?}"
        );
        // The lazy `''` page no longer merges into the root.
        assert!(
            !handled
                .iter()
                .any(|(f, t, _)| f == "page:/" && t.ends_with("ItemsPageComponent"))
        );
        let nav = nav_edges(&g, glia_code_domain::edge_category::NAVIGATES_TO);
        assert_eq!(
            nav,
            // The link sits on the MODULE: these parses run no component
            // extractor, so the page lift has no COMPONENT to move it to.
            [(
                "src::app::features::items::items-page.component".to_string(),
                "page:/app/items/:itemId".to_string(),
                "param".to_string()
            )]
        );
        assert_eq!(dead_links(&g), ["/404"]);
        assert_eq!(
            nav_stats(&nav_fixture()).marker().as_deref(),
            Some(
                "[nav-mounts] lazy=1 (import=1 string=0 ngmodule_hop=0 dynamic=0) \
                 unresolved=0 no_table=0 tables=2 routes=6 mounted=2 copies=0 roots=1 \
                 capped=0 links_rewritten=0"
            )
        );

        // A second level, a relative redirect inside a lazy table, and a spec
        // nothing in the graph answers.
        let app = NAV_APP.replace(
            "  { path: '**'",
            "  { path: 'ext', loadChildren: () => import('@scope/ext').then(m => m.EXT_ROUTES) },\n  { path: '**'",
        );
        let items = NAV_ITEMS.replace(
            "];",
            "  { path: ':itemId/notes', loadChildren: () => import('./notes/routes').then(m => m.NOTE_ROUTES) },\n  { path: 'old', redirectTo: '' },\n];",
        );
        let notes = "export const NOTE_ROUTES = [\n  { path: '', component: NotesComponent },\n  { path: ':noteId', component: NoteComponent },\n];\n";
        let mut files = nav_fixture();
        files[0].1 = &app;
        files[2].1 = &items;
        files.push(("src/app/features/items/notes/routes.ts", notes));
        let g = nav_build(&files);
        assert_eq!(
            routes(&g),
            [
                "page:/",
                "page:/**",
                "page:/app",
                "page:/app/items",
                "page:/app/items/:itemId",
                "page:/app/items/:itemId/notes",
                "page:/app/items/:itemId/notes/:noteId",
                "page:/app/items/old",
                "page:/ext",
            ]
        );
        let nav = nav_edges(&g, glia_code_domain::edge_category::NAVIGATES_TO);
        assert!(
            nav.contains(&(
                "page:/app/items/old".to_string(),
                "page:/app/items".to_string(),
                "exact".to_string()
            )),
            "{nav:?}"
        );
        assert_eq!(
            nav_stats(&files).marker().as_deref(),
            Some(
                "[nav-mounts] lazy=3 (import=3 string=0 ngmodule_hop=0 dynamic=0) \
                 unresolved=1 no_table=0 tables=3 routes=11 mounted=6 copies=0 roots=1 \
                 capped=0 links_rewritten=1"
            )
        );
    }

    /// The NgModule form (`items.module` importing its routing module, which
    /// holds the table) and the legacy `'<spec>#<export>'` string.
    #[test]
    fn nav_ngmodule_hop_and_legacy_string() {
        let app = "export const routes = [\n  { path: 'items', loadChildren: () => import('./items/items.module').then(m => m.ItemsModule) },\n  { path: 'old', loadChildren: './old/old.module#OldModule' },\n];\n";
        let module = "import { NgModule } from '@angular/core';\nimport { RouterModule } from '@angular/router';\nimport { ItemsRoutingModule } from './items-routing.module';\n\n@NgModule({ imports: [ItemsRoutingModule] })\nexport class ItemsModule {}\n";
        let routing = "import { NgModule } from '@angular/core';\nimport { RouterModule } from '@angular/router';\n\nconst routes = [{ path: ':id', component: ItemComponent }];\n\n@NgModule({ imports: [RouterModule.forChild(routes)] })\nexport class ItemsRoutingModule {}\n";
        let old = "const routes = [{ path: 'list', component: OldListComponent }];\n";
        let files = [
            ("src/app.routes.ts", app),
            ("src/items/items.module.ts", module),
            ("src/items/items-routing.module.ts", routing),
            ("src/old/old.module.ts", old),
        ];
        let g = nav_build(&files);
        assert_eq!(
            routes(&g),
            [
                "page:/items",
                "page:/items/:id",
                "page:/old",
                "page:/old/list"
            ]
        );
        assert_eq!(
            nav_stats(&files).marker().as_deref(),
            Some(
                "[nav-mounts] lazy=2 (import=1 string=1 ngmodule_hop=1 dynamic=0) \
                 unresolved=0 no_table=0 tables=3 routes=4 mounted=2 copies=0 roots=1 \
                 capped=0 links_rewritten=0"
            )
        );
    }

    /// One table mounted twice is cloned under each prefix; a mount cycle
    /// reaches no root and keeps every page at its own path.
    #[test]
    fn nav_two_mounts_clone_and_cycles_keep_paths() {
        let app = "export const routes = [\n  { path: 'a', loadChildren: () => import('./shared/routes').then(m => m.R) },\n  { path: 'b', loadChildren: () => import('./shared/routes').then(m => m.R) },\n];\n";
        let shared = "export const R = [{ path: 'x', component: X }];\n";
        let g = nav_build(&[("src/app.ts", app), ("src/shared/routes.ts", shared)]);
        assert_eq!(routes(&g), ["page:/a", "page:/a/x", "page:/b", "page:/b/x"]);
        let one = "export const A = [{ path: 'p', component: P }, { path: 'two', loadChildren: () => import('./two').then(m => m.B) }];\n";
        let two = "export const B = [{ path: 'q', component: Q }, { path: 'one', loadChildren: () => import('./one').then(m => m.A) }];\n";
        let g = nav_build(&[("src/one.ts", one), ("src/two.ts", two)]);
        assert_eq!(routes(&g), ["page:/one", "page:/p", "page:/q", "page:/two"]);
    }

    /// The existing nav fixtures' sources hold no `loadChildren`: built with
    /// the provisional slot and the pass, every page qname, node, edge and
    /// dead link is byte-equal to the BASE build that minted them final.
    #[test]
    fn nav_root_tables_keep_their_paths() {
        let angular = [
            (
                "src/app/app.routes.ts",
                include_str!(
                    "../../../bench/substrate-gap/fixtures/nav-angular-routes/src/app/app.routes.ts"
                ),
            ),
            (
                "src/app/home.component.ts",
                include_str!(
                    "../../../bench/substrate-gap/fixtures/nav-angular-routes/src/app/home.component.ts"
                ),
            ),
            (
                "src/app/admin-users.component.ts",
                include_str!(
                    "../../../bench/substrate-gap/fixtures/nav-angular-routes/src/app/admin-users.component.ts"
                ),
            ),
            (
                "src/util.ts",
                include_str!(
                    "../../../bench/substrate-gap/fixtures/nav-angular-routes/src/util.ts"
                ),
            ),
            (
                "src/app/login.component.ts",
                include_str!(
                    "../../../bench/substrate-gap/fixtures/nav-angular-links/src/app/login.component.ts"
                ),
            ),
        ];
        let react = [
            (
                "src/router.tsx",
                include_str!(
                    "../../../bench/substrate-gap/fixtures/nav-react-routes/src/router.tsx"
                ),
            ),
            (
                "src/App.tsx",
                include_str!("../../../bench/substrate-gap/fixtures/nav-react-routes/src/App.tsx"),
            ),
            (
                "src/pages.tsx",
                include_str!(
                    "../../../bench/substrate-gap/fixtures/nav-react-routes/src/pages.tsx"
                ),
            ),
        ];
        let vue = [(
            "src/router.ts",
            include_str!("../../../bench/substrate-gap/fixtures/nav-vue-links/src/router.ts"),
        )];
        for files in [&angular[..], &react[..], &vue[..]] {
            let base = nav_build_with(files, true);
            let now = nav_build(files);
            assert!(!routes(&now).is_empty());
            assert_eq!(routes(&now), routes(&base));
            assert_eq!(now.nodes, base.nodes);
            assert_eq!(now.edges, base.edges);
            assert_eq!(now.unresolved_refs, base.unresolved_refs);
            for n in &now.nodes {
                assert_eq!(
                    now.nav.qname_by_id.get(&n.id),
                    base.nav.qname_by_id.get(&n.id)
                );
                assert_eq!(
                    now.nav.name_by_id.get(&n.id),
                    base.nav.name_by_id.get(&n.id)
                );
            }
        }
    }

    /// A body-only re-stitch of a file holding route tables is refused: its
    /// fresh parse mints the provisional pages the nav mount pass re-keyed.
    #[test]
    fn a_rebody_of_a_lazy_routes_file_is_refused() {
        let mut g = nav_build(&nav_fixture());
        let edited = NAV_ITEMS.replace("ItemsPageComponent }", "ItemsPageComponent, title: 'x' }");
        let got = crate::build::rebuild_ts_bodies(
            &mut g,
            vec![crate::build::BodyEdit::new(nav_parse_with(
                "src/app/features/items/routes.ts",
                &edited,
                false,
            ))],
            ts_source,
        );
        assert_eq!(
            got.err(),
            Some(crate::build::Refused::Unsupported("page navigation"))
        );
        assert!(routes(&g).contains(&"page:/app/items/:itemId".to_string()));
    }
}
