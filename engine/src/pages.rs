//! The page-flow answer (LA.6e): frontend pages, the links between them, dead
//! deep links and unlinked pages, in one call. `glia pages` and the pyo3
//! `PyGraph.page_flow()` are its transports.
//!
//! Read-only over what LA.6a-6d put in the graph:
//!
//! - a **page** is a ROUTE that `repo_graph_graph::nav::is_nav_route` marks,
//!   its path read by `nav_route_path` (both LA.6a's, never re-derived here),
//!   located by the handler it is HANDLED_BY;
//! - a **link** is a `NAVIGATES_TO` edge from anything but a ROUTE. A
//!   route-to-route `NAVIGATES_TO` is a redirect and is shown on its page as
//!   `redirect_to`;
//! - a **dead link** is a `NAVIGATES_TO` entry the resolver left in a graph's
//!   `unresolved_refs` (LA.6a keeps only router-API links there, never plain
//!   `href:` anchors). One the graph could not judge is dropped here: a path a
//!   non-nav ROUTE serves anywhere in the merged graph (a Go or Python backend
//!   route sits in another per-language graph, out of the resolver's
//!   same-graph `server` check). `absorbed_by` names the root catch-all that
//!   swallows it at runtime;
//! - an **unlinked page** is a page no in-repo link or redirect reaches, other
//!   than `/` and a root catch-all. It is a FACT about the repo, never a dead
//!   page: email, bookmarks and external sites deep-link too.
//!
//! Every list is sorted, so two calls over one graph serialise byte-identically.
//! Lines are 1-based ([`crate::Located`]). Dynamic navigations the extractor
//! cannot read are never in any list; the `typescript` / `NAVIGATES_TO` row of
//! `coverage_report` declares them.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use repo_graph_code_domain::endpoint::split_owner;
use repo_graph_code_domain::{CallQualifier, edge_category, node_kind};
use repo_graph_core::{Confidence, NodeId};
use repo_graph_graph::nav::{is_nav_route, nav_route_path};
use repo_graph_graph::{MergedGraph, normalise_http_path};

use crate::answers::Locator;

/// One page: a client-router route.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PageRecord {
    /// The route path as the router table writes it (`/user/:publicId`).
    pub path: String,
    pub route_qname: String,
    /// Qualified name of the component the route is HANDLED_BY; the least
    /// qname when there are several. `None` for a pure redirect.
    pub handler: Option<String>,
    pub handler_file: Option<String>,
    pub handler_line: Option<i64>,
    /// Path of the page this route redirects to (`redirectTo`, `<Navigate>`).
    pub redirect_to: Option<String>,
    /// The whole path is the wildcard (`**`, `*`, `/:pathMatch(.*)*`): the
    /// router's 404 / redirect fallback. A scoped catch-all (`/docs/**`) is an
    /// ordinary page and is `false`.
    pub catchall: bool,
    /// Distinct sources with a `NAVIGATES_TO` into this route, redirects
    /// included.
    pub inbound_links: usize,
}

/// One resolved navigation from a page (or, when the linking file holds more
/// than one component, its MODULE) to a route.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NavLinkRecord {
    pub from_qname: String,
    pub from_file: Option<String>,
    pub from_line: Option<i64>,
    /// Path of the route the link binds to.
    pub to_path: String,
    /// The binding tier: `strong` (exact path), `medium` (parameter or scoped
    /// catch-all, or a plain anchor), `weak` (path-suffix match).
    pub confidence: &'static str,
}

/// A router link no route serves.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DeadLink {
    pub from_qname: String,
    pub from_file: Option<String>,
    pub from_line: Option<i64>,
    /// The link as the source writes it (`/connect`); dynamic segments are
    /// `${...}`.
    pub link: String,
    /// The root catch-all of the same app that takes the link at runtime, with
    /// its redirect: `"/** -> /login"`, or just `"/**"` when it renders a
    /// page. `None` when the app has no catch-all (the router errors).
    pub absorbed_by: Option<String>,
}

/// The whole answer. Serialises as `{pages, links, dead, unlinked}`.
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PageFlow {
    /// Sorted by (path, route qname).
    pub pages: Vec<PageRecord>,
    /// Sorted by (from qname, to path).
    pub links: Vec<NavLinkRecord>,
    /// Sorted by (from qname, link).
    pub dead: Vec<DeadLink>,
    /// Paths of the unlinked pages, sorted; ` @<project>` is appended when the
    /// route carries an LB.4a owner segment, so two apps' `/home` stay apart.
    pub unlinked: Vec<String>,
}

impl PageFlow {
    /// The LA.6e fired_on line each transport prints:
    /// `[pages] surface=<surface> repos=N pages=P links=L dead=D unlinked=U`.
    pub fn marker(&self, surface: &str, repos: usize) -> String {
        format!(
            "[pages] surface={surface} repos={repos} pages={} links={} dead={} unlinked={}",
            self.pages.len(),
            self.links.len(),
            self.dead.len(),
            self.unlinked.len()
        )
    }
}

/// A nav ROUTE, as read once from the graph that first names it.
struct Route<'a> {
    graph: usize,
    qname: &'a str,
    path: &'a str,
    root_catchall: bool,
}

/// **page_flow** (LA.6e): every page, link, dead link and unlinked page of the
/// frontends in `merged` (see the module doc). Read-only.
pub fn page_flow(merged: &MergedGraph) -> PageFlow {
    // Nav routes by id (first graph wins), and every path a server route serves.
    let mut routes: BTreeMap<u64, Route<'_>> = BTreeMap::new();
    let mut server_paths: HashSet<String> = HashSet::new();
    for (gi, g) in merged.graphs.iter().enumerate() {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ROUTE) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else {
                continue;
            };
            let Some(path) = nav_route_path(qname) else {
                continue;
            };
            if !is_nav_route(&n.cells) {
                server_paths.insert(normalise_http_path(path));
                continue;
            }
            routes.entry(n.id.0).or_insert(Route {
                graph: gi,
                qname,
                path,
                root_catchall: is_root_catchall(path),
            });
        }
    }

    let loc = Locator::new(merged);
    let path_of = |id: NodeId| routes.get(&id.0).map(|r| r.path);

    // One pass over the edges, deduped on (from, to, category): a node folded
    // into two per-language graphs can repeat an edge.
    let mut seen: HashSet<(u64, u64, u32)> = HashSet::new();
    let mut handlers: HashMap<u64, Vec<NodeId>> = HashMap::new();
    let mut redirects: HashMap<u64, Vec<&str>> = HashMap::new();
    let mut inbound: HashMap<u64, usize> = HashMap::new();
    let mut links: Vec<(NavLinkRecord, &str)> = Vec::new();
    for e in merged.all_edges() {
        let is_handler = e.category == edge_category::HANDLED_BY;
        if !is_handler && e.category != edge_category::NAVIGATES_TO {
            continue;
        }
        if !seen.insert((e.from.0, e.to.0, e.category.0)) {
            continue;
        }
        if is_handler {
            if routes.contains_key(&e.from.0) {
                handlers.entry(e.from.0).or_default().push(e.to);
            }
            continue;
        }
        let Some(to_path) = path_of(e.to) else {
            continue;
        };
        *inbound.entry(e.to.0).or_default() += 1;
        let from = loc.locate(e.from);
        if from.kind == node_kind::name(node_kind::ROUTE) {
            redirects.entry(e.from.0).or_default().push(to_path);
            continue;
        }
        let to_qname = routes.get(&e.to.0).map_or("", |r| r.qname);
        links.push((
            NavLinkRecord {
                from_qname: from.qname,
                from_file: from.file,
                from_line: from.line,
                to_path: to_path.to_string(),
                confidence: confidence_name(e.confidence),
            },
            to_qname,
        ));
    }
    links.sort_by(|(a, aq), (b, bq)| {
        (a.from_qname.as_str(), a.to_path.as_str(), *aq)
            .cmp(&(b.from_qname.as_str(), b.to_path.as_str(), *bq))
    });
    let links: Vec<NavLinkRecord> = links.into_iter().map(|(l, _)| l).collect();

    let redirect_of = |id: u64| {
        redirects
            .get(&id)
            .and_then(|targets| targets.iter().min())
            .map(|p| p.to_string())
    };

    let mut pages: Vec<PageRecord> = routes
        .iter()
        .map(|(id, r)| {
            let handler = handlers
                .get(id)
                .into_iter()
                .flatten()
                .map(|h| loc.locate(*h))
                .min_by(|a, b| a.qname.cmp(&b.qname));
            PageRecord {
                path: r.path.to_string(),
                route_qname: r.qname.to_string(),
                handler_file: handler.as_ref().and_then(|h| h.file.clone()),
                handler_line: handler.as_ref().and_then(|h| h.line),
                handler: handler.map(|h| h.qname),
                redirect_to: redirect_of(*id),
                catchall: r.root_catchall,
                inbound_links: inbound.get(id).copied().unwrap_or(0),
            }
        })
        .collect();
    pages.sort_by(|a, b| (&a.path, &a.route_qname).cmp(&(&b.path, &b.route_qname)));

    let (dead, served) = dead_links(merged, &loc, &routes, &server_paths, &redirect_of);
    if served > 0 {
        eprintln!("[pages] dead refs a server route serves, dropped: {served}");
    }

    let mut unlinked: Vec<String> = routes
        .iter()
        .filter(|(id, r)| {
            !r.root_catchall
                && !r.path.trim_matches('/').is_empty()
                && inbound.get(*id).copied().unwrap_or(0) == 0
        })
        .map(|(_, r)| match split_owner(r.qname).1 {
            Some(owner) => format!("{} @{owner}", r.path),
            None => r.path.to_string(),
        })
        .collect();
    unlinked.sort();

    PageFlow {
        pages,
        links,
        dead,
        unlinked,
    }
}

/// The dead links of every graph, deduped on (from, link) and sorted, plus how
/// many refs were dropped because a server route serves their path.
fn dead_links(
    merged: &MergedGraph,
    loc: &Locator<'_>,
    routes: &BTreeMap<u64, Route<'_>>,
    server_paths: &HashSet<String>,
    redirect_of: &dyn Fn(u64) -> Option<String>,
) -> (Vec<DeadLink>, usize) {
    let mut served = 0usize;
    let mut keys: BTreeSet<(u64, &str)> = BTreeSet::new();
    let mut dead: Vec<DeadLink> = Vec::new();
    for (gi, g) in merged.graphs.iter().enumerate() {
        for r in &g.unresolved_refs {
            if r.category != edge_category::NAVIGATES_TO || !g.nav.kind_by_id.contains_key(&r.from)
            {
                continue;
            }
            let CallQualifier::Bare(link) = &r.qualifier else {
                continue;
            };
            if link.starts_with("href:") {
                continue;
            }
            let path = link.split(['?', '#']).next().unwrap_or(link);
            if server_paths.contains(&normalise_http_path(path)) {
                served += 1;
                continue;
            }
            if !keys.insert((r.from.0, link.as_str())) {
                continue;
            }
            let from = loc.locate(r.from);
            let absorbed_by = catchall_for(routes, gi, from.file.as_deref()).map(|(id, route)| {
                match redirect_of(id) {
                    Some(to) => format!("{} -> {to}", route.path),
                    None => route.path.to_string(),
                }
            });
            dead.push(DeadLink {
                from_qname: from.qname,
                from_file: from.file,
                from_line: from.line,
                link: link.clone(),
                absorbed_by,
            });
        }
    }
    dead.sort_by(|a, b| (&a.from_qname, &a.link).cmp(&(&b.from_qname, &b.link)));
    (dead, served)
}

/// The root catch-all of graph `gi` that serves a link from `file`: the one
/// whose LB.4a owner segment encloses the file (the longest), else the least
/// by (path, qname).
fn catchall_for<'r, 'a>(
    routes: &'r BTreeMap<u64, Route<'a>>,
    gi: usize,
    file: Option<&str>,
) -> Option<(u64, &'r Route<'a>)> {
    let mut candidates: Vec<(u64, &Route<'a>)> = routes
        .iter()
        .filter(|(_, r)| r.graph == gi && r.root_catchall)
        .map(|(id, r)| (*id, r))
        .collect();
    candidates.sort_by(|(_, a), (_, b)| (a.path, a.qname).cmp(&(b.path, b.qname)));
    let encloses = |owner: &str| {
        file.is_some_and(|f| {
            f == owner
                || f.strip_prefix(owner)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    };
    // `min_by_key` keeps the first of equal keys, so the sort breaks ties.
    let owned = candidates
        .iter()
        .filter_map(|c| split_owner(c.1.qname).1.filter(|o| encloses(o)).map(|o| (o.len(), *c)))
        .min_by_key(|(len, _)| std::cmp::Reverse(*len))
        .map(|(_, c)| c);
    owned.or_else(|| candidates.first().copied())
}

fn confidence_name(c: Confidence) -> &'static str {
    match c {
        Confidence::Strong => "strong",
        Confidence::Medium => "medium",
        Confidence::Weak => "weak",
    }
}

/// The whole path is one wildcard segment: `*`, `**`, `[...x]`, `[[...x]]`,
/// `:x*`, `:x+`, `:x(.*)*`.
///
/// STOPGAP: this mirrors the private `wildcard` / `classify` pair in
/// `graph/src/nav.rs` (its `Shape::Root`), which the graph crate does not
/// export and this packet may not edit. Removal: export a
/// `repo_graph_graph::nav::is_root_catchall(path)` from that classifier, call
/// it here and delete this function and its test.
fn is_root_catchall(path: &str) -> bool {
    let segs: Vec<&str> = path.trim().split('/').filter(|s| !s.is_empty()).collect();
    let [seg] = segs.as_slice() else {
        return false;
    };
    let seg = *seg;
    seg == "*"
        || seg == "**"
        || (seg.len() > 7 && seg.starts_with("[[...") && seg.ends_with("]]"))
        || (seg.len() > 5 && seg.starts_with("[...") && seg.ends_with(']'))
        || (seg.len() > 2 && seg.starts_with(':') && (seg.ends_with('*') || seg.ends_with('+')))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_catch_alls_match_the_graph_s_classifier() {
        // The `Shape::Root` cases of graph/src/nav.rs's `catch_alls_split_root_from_scoped`.
        for root in ["/**", "*", "/:pathMatch(.*)*", "/[...all]", "/[[...all]]", "/:rest+"] {
            assert!(is_root_catchall(root), "{root}");
        }
        // Scoped catch-alls are pages; parameters and mid-path stars are not wildcards.
        for page in [
            "/docs/**",
            "/docs/[...slug]",
            "/u/:id/:rest(.*)*",
            "/users/:id",
            "/a/*/b",
            "/p/[id]",
            "/",
            "",
            "/:id",
        ] {
            assert!(!is_root_catchall(page), "{page}");
        }
    }

    #[test]
    fn an_empty_graph_has_no_pages() {
        let flow = page_flow(&MergedGraph::new(Vec::new()));
        assert_eq!(
            serde_json::to_string(&flow).expect("serialises"),
            r#"{"pages":[],"links":[],"dead":[],"unlinked":[]}"#
        );
        assert_eq!(
            flow.marker("cli", 1),
            "[pages] surface=cli repos=1 pages=0 links=0 dead=0 unlinked=0"
        );
    }
}
