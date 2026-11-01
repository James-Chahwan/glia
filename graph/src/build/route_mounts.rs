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
//! Deterministic: urlconf modules and facts in `g.nodes` order of their
//! MODULE (facts in record order), roots by qname, BTree slots; the clones
//! append in provisional order.

use std::collections::{BTreeMap, BTreeSet};

use glia_code_domain::endpoint::{abs_path, join_path};
use glia_code_domain::{
    ImportStmt, ImportTarget, IncludeTarget, Mount, NavFact, UnresolvedRef, node_kind,
};
use glia_core::NodeId;

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
}

impl RouteMountStats {
    /// `[route-mounts] lang=<l> includes=<i> spreads=<s> dynamic=<d>
    /// unresolved=<u> modules=<m> routes=<r> mounted=<k> copies=<c> roots=<o>
    /// capped=<x>`, once per graph with a route module.
    pub(crate) fn marker(&self, lang: &str) -> Option<String> {
        (self.modules > 0).then(|| {
            format!(
                "[route-mounts] lang={lang} includes={} spreads={} dynamic={} unresolved={} \
                 modules={} routes={} mounted={} copies={} roots={} capped={}",
                self.includes,
                self.spreads,
                self.dynamic,
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

    // Route modules in `g.nodes` order, with their facts.
    let mut route_modules: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut facts: Vec<(String, &NavFact)> = Vec::new();
    let mut modules: Vec<(String, NodeId)> = Vec::new();
    for n in &g.nodes {
        if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::MODULE) {
            continue;
        }
        let Some(q) = g.nav.qname_by_id.get(&n.id) else {
            continue;
        };
        modules.push((q.clone(), n.id));
        let own: Vec<&NavFact> = g
            .nav
            .nav_facts
            .get(&n.id)
            .map(|fs| {
                fs.iter()
                    .filter(|f| matches!(f, NavFact::IncludeMount { .. }))
                    .collect()
            })
            .unwrap_or_default();
        if !own.is_empty() && seen.insert(q.clone()) {
            route_modules.push(q.clone());
        }
        facts.extend(own.into_iter().map(|f| (q.clone(), f)));
    }
    for p in &provisionals {
        if let Mount::Module { module, .. } = &p.base
            && seen.insert(module.clone())
        {
            route_modules.push(module.clone());
        }
    }
    if route_modules.is_empty() {
        return stats;
    }
    stats.modules = route_modules.len();
    stats.routes = provisionals.len();
    modules.sort_by(|a, b| a.0.cmp(&b.0));

    // Each include resolved to a flow into its target's slot.
    let index = ModuleIndex { g, modules };
    let mut includes: Vec<Flow> = Vec::new();
    let mut included: BTreeSet<String> = BTreeSet::new();
    for (includer, fact) in &facts {
        let NavFact::IncludeMount {
            target,
            prefix,
            via,
            ..
        } = fact
        else {
            continue;
        };
        let resolved = match target {
            IncludeTarget::Dynamic(_) => {
                stats.dynamic += 1;
                continue;
            }
            IncludeTarget::Spec(dotted) => index.by_tail(&dotted.replace('.', "::"), includer),
            IncludeTarget::Import { local } => index.by_import(imports, includer, local),
        };
        if via == VIA_SPREAD {
            stats.spreads += 1;
        } else {
            stats.includes += 1;
        }
        let Some(module) = resolved else {
            stats.unresolved += 1;
            continue;
        };
        if module != *includer {
            included.insert(module.clone());
        }
        includes.push(Flow {
            target: Slot::Module(module, DJANGO_VAR.to_string()),
            mount: Mount::Module {
                module: includer.clone(),
                var: DJANGO_VAR.to_string(),
                suffix: prefix.clone(),
            },
        });
    }

    // Roots first (by qname), then the includes in fact order.
    let roots: BTreeSet<&String> = route_modules
        .iter()
        .filter(|m| !included.contains(*m))
        .collect();
    stats.roots = roots.len();
    let mut flows: Vec<Flow> = roots
        .into_iter()
        .map(|m| Flow {
            target: Slot::Module(m.clone(), DJANGO_VAR.to_string()),
            mount: Mount::Const(String::new()),
        })
        .collect();
    flows.extend(includes);
    let (slots, capped) = fixpoint(&flows, &BTreeMap::new());
    stats.capped = capped;

    let mut plans: Vec<Vec<Final>> = Vec::with_capacity(provisionals.len());
    for p in &provisionals {
        let (module, var) = match &p.base {
            Mount::Module { module, var, .. } => (module.as_str(), var.as_str()),
            _ => ("", ""),
        };
        let prefixes = slots
            .get(&Slot::Module(module.to_string(), var.to_string()))
            .filter(|s| !s.is_empty());
        let paths: BTreeSet<String> = match prefixes {
            Some(set) => {
                if included.contains(module) {
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
                "[route-mounts] lang=python includes=3 spreads=1 dynamic=0 unresolved=0 \
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
}
