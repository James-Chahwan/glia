//! Frontend page flow (LA.6a): which ROUTEs are client-router navigation
//! targets ([`is_nav_route`]), the path each serves ([`nav_route_path`]), and
//! the `NAVIGATES_TO` resolver that binds navigation links to them.
//!
//! Parsers extract, the graph crate resolves (v0.4.3b): matching a link
//! literal against the route table (placeholder folding, catch-alls, child
//! tables whose parent prefix is in another file) is resolution, so it runs
//! here, from `calls::resolve_refs`, after every other ref has bound.
//!
//! # The link contract
//!
//! Every emitter (route tables LA.6b, link sites LA.6c, Next.js pages LA.6d)
//! writes one ref per navigation:
//!
//! ```text
//! UnresolvedRef { from: <MODULE id of the linking file | nav ROUTE id for a redirect>,
//!                 from_module: <MODULE id of the file>,
//!                 qualifier: CallQualifier::Bare(<link>),
//!                 category: edge_category::NAVIGATES_TO }
//! <link> = "/path"       router-API link (routerLink, <Link to>, navigate(), router.push,
//!                        redirectTo, an origin share link): CAN be reported dead
//! <link> = "href:/path"  plain <a href> / location.href: binds like any link but is NEVER
//!                        reported dead (an SPA anchor may target a server path: an OAuth
//!                        start, logout, a download)
//! ```
//!
//! Dynamic segments are written `${...}`; the emitter strips query and
//! fragment (and this module strips any `?` / `#` tail that slips through).
//! The category is the discriminator and the qualifier stays `Bare`, so the
//! persisted `unresolved_refs` layout is unchanged (no new `CallQualifier`
//! variant, no format bump).
//!
//! # Matching
//!
//! Both sides go through [`normalise_http_path`] (`:id`, `{id}`, `[id]`,
//! `${...}` all fold to `{}`). The first tier with any hit wins; each hit is
//! one edge, deduped on `(from, to)`:
//!
//! | tier   | rule                                                            | confidence |
//! |--------|-----------------------------------------------------------------|------------|
//! | exact  | the link equals a parameter-free route path                     | Strong     |
//! | param  | equal segment count; a route `{}` takes any segment, a link `{}` only a route `{}` | Medium |
//! | scoped | the link sits under a SCOPED catch-all (`/docs/**`, `/docs/[...slug]`) | Medium |
//! | suffix | the route's segments are a proper suffix of the link's (a child table whose parent prefix is in another file) | Weak |
//!
//! A plain-anchor (`href:`) edge is capped at Medium. A ROOT catch-all (`**`,
//! `*`, `/:pathMatch(.*)*`: the whole path is the wildcard) is the 404 /
//! redirect fallback and is never a target — it would otherwise absorb every
//! dead link. Under LB.4a owner segments, routes of the linking file's project
//! are tried first, then every route.
//!
//! # Dead links
//!
//! An unmatched router link is pushed onto `g.unresolved_refs` unchanged:
//! that IS the dead-link record, persisted by the store's `unresolved_refs`
//! and restored by `load_from_gmap`. A link is never reported dead when the
//! graph has no nav route at all (`no_router`: a server-rendered repo's links
//! are unjudgeable), when a same-graph server ROUTE serves its path
//! (`server`), when it is a plain anchor (`href_unmatched`), or when its
//! `from` is not a node of the graph (`orphan`).
//!
//! # The lift
//!
//! A page is a component, not a file. `lift_nav_endpoints` moves a MODULE
//! endpoint (a link's `from`, a dead link's `from`, a nav ROUTE's HANDLED_BY
//! target) onto the file's page component when it has exactly one — a node
//! whose roles ([`roles_in`], LB.3) include COMPONENT. Page flow then reads
//! `page -NAVIGATES_TO-> ROUTE -HANDLED_BY-> page` end to end.
//!
//! Determinism: the index is built in `g.nodes` order, every HashMap / HashSet
//! is lookup-only, and edges are appended in ref order.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::endpoint::split_owner;
use repo_graph_code_domain::{CallQualifier, UnresolvedRef, cell_type, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, EdgeCategoryId, NodeId};

use crate::calls::position_file;
use crate::resolvers::{normalise_http_path, weakest};
use crate::roles::roles_in;
use crate::types::RepoGraph;

/// A ROUTE node tagged `provenance: nav_route` by a client-router extractor
/// (react-router / Angular Router / vue-router / go_router). It is a browser
/// navigation target, not a server endpoint: the HTTP resolver never pairs a
/// client call with it, and it is what a `NAVIGATES_TO` link binds to.
///
/// Cheap substring test: the payload is written by us (the extractors crate's
/// `nav_route_origin_cell`), not by user JSON, which keeps serde_json out of
/// the graph crate. Both payload spellings are accepted so a future
/// Text-payload emitter still marks.
pub fn is_nav_route(cells: &[Cell]) -> bool {
    cells.iter().any(|c| {
        c.kind == cell_type::ORIGIN
            && matches!(&c.payload, CellPayload::Json(j) | CellPayload::Text(j)
                        if j.contains("\"provenance\":\"nav_route\""))
    })
}

/// The request path of a ROUTE qname, in every shape the tree emits: the
/// LB.4a owner segment (` @<project>`) is split off first, then `page:<p>`
/// (LB.4c client-router pages), `route:<p>` (go / ts_routes) or the legacy
/// `<METHOD> <p>` (an upper-case method, a path starting with `/`). `None` for
/// any other qname. The path is returned raw, un-normalised.
pub fn nav_route_path(qname: &str) -> Option<&str> {
    let (q, _owner) = split_owner(qname);
    if let Some(p) = q.strip_prefix("page:") {
        return Some(p);
    }
    if let Some(p) = q.strip_prefix("route:") {
        return Some(p);
    }
    let (method, path) = q.split_once(' ')?;
    let is_method = !method.is_empty() && method.bytes().all(|b| b.is_ascii_uppercase());
    (is_method && path.starts_with('/')).then_some(path)
}

/// Per-graph tally behind the `[nav]` marker. `resolved` is the sum of the
/// four tier counters, counted per ref before the `(from, to)` dedupe.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct NavStats {
    pub(crate) links: usize,
    pub(crate) exact: usize,
    pub(crate) param: usize,
    pub(crate) scoped: usize,
    pub(crate) suffix: usize,
    pub(crate) dead: usize,
    pub(crate) catchall: usize,
    pub(crate) server: usize,
    pub(crate) no_router: usize,
    pub(crate) href_unmatched: usize,
    pub(crate) orphan: usize,
    pub(crate) lifted: usize,
}

impl NavStats {
    /// True when the graph saw a link or had an endpoint lifted: the only
    /// graphs that print the marker.
    pub(crate) fn fired(&self) -> bool {
        self.links > 0 || self.lifted > 0
    }

    /// `[nav] links=L resolved=R (exact=E param=P scoped=C suffix=S) dead=D
    /// catchall=A server=V no_router=N href_unmatched=H orphan=O lifted=F`.
    pub(crate) fn marker(&self) -> String {
        format!(
            "[nav] links={} resolved={} (exact={} param={} scoped={} suffix={}) dead={} \
             catchall={} server={} no_router={} href_unmatched={} orphan={} lifted={}",
            self.links,
            self.exact + self.param + self.scoped + self.suffix,
            self.exact,
            self.param,
            self.scoped,
            self.suffix,
            self.dead,
            self.catchall,
            self.server,
            self.no_router,
            self.href_unmatched,
            self.orphan,
            self.lifted,
        )
    }

    fn count(&mut self, tier: Tier) {
        match tier {
            Tier::Exact => self.exact += 1,
            Tier::Param => self.param += 1,
            Tier::Scoped => self.scoped += 1,
            Tier::Suffix => self.suffix += 1,
        }
    }
}

/// One normalised path segment.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Lit(String),
    /// `{}` after [`normalise_http_path`].
    Param,
}

/// Segments of an already-normalised path.
fn segs_of(norm: &str) -> Vec<Seg> {
    norm.split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            if s == "{}" {
                Seg::Param
            } else {
                Seg::Lit(s.to_string())
            }
        })
        .collect()
}

/// `route` matches `link` segment by segment: a route `{}` takes anything, a
/// link `{}` (a dynamic value) only a route `{}`.
fn segs_match(route: &[Seg], link: &[Seg]) -> bool {
    route.len() == link.len()
        && route.iter().zip(link).all(|(r, l)| match (r, l) {
            (Seg::Param, _) => true,
            (Seg::Lit(a), Seg::Lit(b)) => a == b,
            (Seg::Lit(_), Seg::Param) => false,
        })
}

/// A nav ROUTE's path, classified on its RAW text: `normalise_http_path`
/// folds `:x*` and `[...x]` to `{}` and keeps `*` / `**` literal, so a
/// catch-all is only recognisable before normalising.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Shape {
    /// No trailing wildcard.
    Plain(Vec<Seg>),
    /// A trailing wildcard under a non-empty prefix: `/docs/**`,
    /// `/docs/[...slug]`, `/docs/:slug*`. `optional` = it also serves the bare
    /// prefix (zero extra segments).
    Scoped { prefix: Vec<Seg>, optional: bool },
    /// The whole path is the wildcard: `**`, `*`, `/:pathMatch(.*)*`.
    Root,
}

/// `Some(optional)` when `seg` is a catch-all segment: `*`, `**`,
/// `[[...x]]`, `:x*` and `:x(.*)*` also match zero segments; `[...x]` and
/// `:x+` need at least one.
fn wildcard(seg: &str) -> Option<bool> {
    if seg == "*" || seg == "**" {
        return Some(true);
    }
    if let Some(inner) = seg.strip_prefix("[[...") {
        return (inner.len() > 2 && inner.ends_with("]]")).then_some(true);
    }
    if let Some(inner) = seg.strip_prefix("[...") {
        return (inner.len() > 1 && inner.ends_with(']')).then_some(false);
    }
    if seg.len() > 2 && seg.starts_with(':') {
        if seg.ends_with('*') {
            return Some(true);
        }
        if seg.ends_with('+') {
            return Some(false);
        }
    }
    None
}

fn classify(raw: &str) -> Shape {
    let segs: Vec<&str> = raw.trim().split('/').filter(|s| !s.is_empty()).collect();
    if let Some((last, before)) = segs.split_last()
        && let Some(optional) = wildcard(last)
    {
        if before.is_empty() {
            return Shape::Root;
        }
        return Shape::Scoped {
            prefix: segs_of(&normalise_http_path(&before.join("/"))),
            optional,
        };
    }
    Shape::Plain(segs_of(&normalise_http_path(raw)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    Exact,
    Param,
    Scoped,
    Suffix,
}

impl Tier {
    fn confidence(self) -> Confidence {
        match self {
            Tier::Exact => Confidence::Strong,
            Tier::Param | Tier::Scoped => Confidence::Medium,
            Tier::Suffix => Confidence::Weak,
        }
    }
}

/// One matchable nav ROUTE (root catch-alls are never targets and are only
/// counted).
struct NavRoute {
    id: NodeId,
    owner: Option<String>,
    shape: Shape,
}

/// The graph's nav-route table, built once per graph in `g.nodes` order.
#[derive(Default)]
struct NavIndex {
    routes: Vec<NavRoute>,
    /// Parameter-free normalised path -> `routes` indices.
    exact: HashMap<String, Vec<usize>>,
    /// Segment count -> `routes` indices of the `Plain` routes.
    by_len: HashMap<usize, Vec<usize>>,
    any_nav: bool,
    root_catchall: bool,
    /// Normalised paths of every same-graph ROUTE WITHOUT the nav ORIGIN.
    server_paths: HashSet<String>,
    /// Distinct owner segments on nav routes, in first-seen order.
    owners: Vec<String>,
}

impl NavIndex {
    fn build(g: &RepoGraph) -> Self {
        let mut idx = NavIndex::default();
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
                idx.server_paths.insert(normalise_http_path(path));
                continue;
            }
            idx.any_nav = true;
            let owner = split_owner(qname).1.map(str::to_string);
            if let Some(o) = &owner
                && !idx.owners.contains(o)
            {
                idx.owners.push(o.clone());
            }
            let shape = classify(path);
            let at = idx.routes.len();
            match &shape {
                Shape::Root => {
                    idx.root_catchall = true;
                    continue;
                }
                Shape::Scoped { .. } => {}
                Shape::Plain(segs) => {
                    if segs.iter().all(|s| matches!(s, Seg::Lit(_))) {
                        idx.exact
                            .entry(normalise_http_path(path))
                            .or_default()
                            .push(at);
                    }
                    idx.by_len.entry(segs.len()).or_default().push(at);
                }
            }
            idx.routes.push(NavRoute {
                id: n.id,
                owner,
                shape,
            });
        }
        idx
    }

    /// The routes `link` reaches from the first tier with any hit, restricted
    /// to routes of `owner` when one is given. `None` when no tier hits.
    fn lookup(&self, norm: &str, link: &[Seg], owner: Option<&str>) -> Option<(Tier, Vec<NodeId>)> {
        let ok = |at: &usize| owner.is_none_or(|o| self.routes[*at].owner.as_deref() == Some(o));
        let ids = |hits: Vec<usize>| -> Vec<NodeId> {
            hits.into_iter().map(|at| self.routes[at].id).collect()
        };

        let exact: Vec<usize> = self
            .exact
            .get(norm)
            .into_iter()
            .flatten()
            .copied()
            .filter(ok)
            .collect();
        if !exact.is_empty() {
            return Some((Tier::Exact, ids(exact)));
        }
        let param: Vec<usize> = self
            .by_len
            .get(&link.len())
            .into_iter()
            .flatten()
            .copied()
            .filter(ok)
            .filter(|&at| matches!(&self.routes[at].shape, Shape::Plain(segs) if segs_match(segs, link)))
            .collect();
        if !param.is_empty() {
            return Some((Tier::Param, ids(param)));
        }
        let scoped: Vec<usize> = (0..self.routes.len())
            .filter(ok)
            .filter(|&at| match &self.routes[at].shape {
                Shape::Scoped { prefix, optional } => {
                    (link.len() > prefix.len() || (*optional && link.len() == prefix.len()))
                        && segs_match(prefix, &link[..prefix.len()])
                }
                _ => false,
            })
            .collect();
        if !scoped.is_empty() {
            return Some((Tier::Scoped, ids(scoped)));
        }
        let suffix: Vec<usize> = (0..self.routes.len())
            .filter(ok)
            .filter(|&at| match &self.routes[at].shape {
                Shape::Plain(segs) => {
                    !segs.is_empty()
                        && segs.len() < link.len()
                        && segs.iter().any(|s| matches!(s, Seg::Lit(_)))
                        && segs_match(segs, &link[link.len() - segs.len()..])
                }
                _ => false,
            })
            .collect();
        (!suffix.is_empty()).then(|| (Tier::Suffix, ids(suffix)))
    }

    /// The project a ref links from: a redirect's own ROUTE owner, else the
    /// longest nav-route owner enclosing the linking file (the `http_owner`
    /// rule, segment-bounded). `None` when the index has no owners.
    fn owner_of(
        &self,
        g: &RepoGraph,
        pos: &HashMap<NodeId, usize>,
        r: &UnresolvedRef,
    ) -> Option<String> {
        if self.owners.is_empty() {
            return None;
        }
        if g.nav.kind_by_id.get(&r.from) == Some(&node_kind::ROUTE)
            && let Some(o) = g
                .nav
                .qname_by_id
                .get(&r.from)
                .and_then(|q| split_owner(q).1)
        {
            return Some(o.to_string());
        }
        let file = [r.from_module, r.from]
            .iter()
            .filter_map(|id| pos.get(id))
            .find_map(|&at| position_file(&g.nodes[at]))?;
        let file = owner_escaped(&file);
        self.owners
            .iter()
            .filter(|o| {
                file == o.as_str()
                    || file
                        .strip_prefix(o.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
            .cloned()
    }
}

/// A file path in the escaping `http_owner` writes owner segments with
/// (whitespace and `%` percent-escaped), so a path compares with an owner.
fn owner_escaped(path: &str) -> Cow<'_, str> {
    if !path.chars().any(|c| c.is_whitespace() || c == '%') {
        return Cow::Borrowed(path);
    }
    let mut out = String::with_capacity(path.len() + 8);
    for c in path.chars() {
        if c.is_whitespace() || c == '%' {
            let mut buf = [0u8; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// `(plain_anchor, path)`: the `href:` tier marker stripped, then any query /
/// fragment tail.
fn split_link(raw: &str) -> (bool, &str) {
    let (href, rest) = match raw.strip_prefix("href:") {
        Some(p) => (true, p),
        None => (false, raw),
    };
    (href, rest.split(['?', '#']).next().unwrap_or(rest))
}

/// Bind every `NAVIGATES_TO` ref in `refs` (see the module doc): an edge per
/// matched route, a dead link kept in `g.unresolved_refs`, everything else
/// dropped and counted.
pub(crate) fn resolve_nav_links(g: &mut RepoGraph, refs: &[&UnresolvedRef]) -> NavStats {
    let mut s = NavStats::default();
    if refs.is_empty() {
        return s;
    }
    let idx = NavIndex::build(g);
    let pos: HashMap<NodeId, usize> = if idx.owners.is_empty() {
        HashMap::new()
    } else {
        g.nodes
            .iter()
            .enumerate()
            .map(|(at, n)| (n.id, at))
            .collect()
    };
    let mut linked: HashSet<(NodeId, NodeId)> = HashSet::new();
    let mut dead: HashSet<(NodeId, String)> = HashSet::new();
    for &r in refs {
        s.links += 1;
        // A ref outside the contract (a non-Bare qualifier names no link) or
        // from a node the graph does not hold cannot be placed.
        let CallQualifier::Bare(raw) = &r.qualifier else {
            s.orphan += 1;
            continue;
        };
        if !g.nav.kind_by_id.contains_key(&r.from) {
            s.orphan += 1;
            continue;
        }
        let (href, path) = split_link(raw);
        let norm = normalise_http_path(path);
        let link = segs_of(&norm);
        let owner = idx.owner_of(g, &pos, r);
        let hit = owner
            .as_deref()
            .and_then(|o| idx.lookup(&norm, &link, Some(o)))
            .or_else(|| idx.lookup(&norm, &link, None));
        match hit {
            Some((tier, targets)) => {
                s.count(tier);
                let confidence = if href {
                    weakest(tier.confidence(), Confidence::Medium)
                } else {
                    tier.confidence()
                };
                for to in targets {
                    if linked.insert((r.from, to)) {
                        g.edges.push(Edge {
                            from: r.from,
                            to,
                            category: edge_category::NAVIGATES_TO,
                            confidence,
                            cells: Vec::new(),
                        });
                    }
                }
            }
            None if !idx.any_nav => s.no_router += 1,
            None if idx.server_paths.contains(&norm) => s.server += 1,
            None if href => s.href_unmatched += 1,
            None => {
                s.dead += 1;
                if idx.root_catchall {
                    s.catchall += 1;
                }
                // The same link twice from one file is one dead link.
                if dead.insert((r.from, raw.clone())) {
                    g.unresolved_refs.push(r.clone());
                }
            }
        }
    }
    s
}

/// Which end of an edge the lift rewrites.
#[derive(Clone, Copy)]
enum End {
    From,
    To,
}

/// Move MODULE endpoints of page flow onto the file's sole page component
/// (see the module doc). Returns how many endpoints moved. Untouched edges
/// keep their bytes and order; a rewritten edge that now duplicates another
/// `(from, to, category)` is dropped.
pub(crate) fn lift_nav_endpoints(g: &mut RepoGraph) -> usize {
    let is_module = |id: &NodeId| g.nav.kind_by_id.get(id) == Some(&node_kind::MODULE);
    let is_route = |id: &NodeId| g.nav.kind_by_id.get(id) == Some(&node_kind::ROUTE);
    let mut slots: Vec<(usize, End)> = g
        .edges
        .iter()
        .enumerate()
        .filter_map(|(at, e)| {
            if e.category == edge_category::NAVIGATES_TO && is_module(&e.from) {
                Some((at, End::From))
            } else if e.category == edge_category::HANDLED_BY
                && is_route(&e.from)
                && is_module(&e.to)
            {
                Some((at, End::To))
            } else {
                None
            }
        })
        .collect();
    let ref_slots: Vec<usize> = g
        .unresolved_refs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.category == edge_category::NAVIGATES_TO && is_module(&r.from))
        .map(|(at, _)| at)
        .collect();
    if slots.is_empty() && ref_slots.is_empty() {
        return 0;
    }

    let pos: HashMap<NodeId, usize> = g
        .nodes
        .iter()
        .enumerate()
        .map(|(at, n)| (n.id, at))
        .collect();
    let cells_of = |id: &NodeId| {
        pos.get(id)
            .map_or(&[][..], |&at| g.nodes[at].cells.as_slice())
    };
    // A HANDLED_BY lifts only when its ROUTE is a nav page.
    slots
        .retain(|&(at, end)| matches!(end, End::From) || is_nav_route(cells_of(&g.edges[at].from)));

    let mut page_of: HashMap<NodeId, Option<NodeId>> = HashMap::new();
    let mut sole_page = |module: NodeId| -> Option<NodeId> {
        *page_of.entry(module).or_insert_with(|| {
            let mut found: Option<NodeId> = None;
            for child in g.nav.children_of.get(&module).into_iter().flatten() {
                let kind = g.nav.kind_by_id.get(child).copied();
                if !roles_in(kind, cells_of(child)).contains(&node_kind::COMPONENT) {
                    continue;
                }
                match found {
                    Some(f) if f == *child => {}
                    Some(_) => return None,
                    None => found = Some(*child),
                }
            }
            found
        })
    };
    let edge_moves: Vec<(usize, End, NodeId)> = slots
        .iter()
        .filter_map(|&(at, end)| {
            let e = &g.edges[at];
            let module = match end {
                End::From => e.from,
                End::To => e.to,
            };
            sole_page(module).map(|page| (at, end, page))
        })
        .collect();
    let ref_moves: Vec<(usize, NodeId)> = ref_slots
        .iter()
        .filter_map(|&at| sole_page(g.unresolved_refs[at].from).map(|page| (at, page)))
        .collect();

    let lifted = edge_moves.len() + ref_moves.len();
    for (at, page) in ref_moves {
        g.unresolved_refs[at].from = page;
    }
    if edge_moves.is_empty() {
        return lifted;
    }
    let mut rewritten = vec![false; g.edges.len()];
    for (at, end, page) in edge_moves {
        match end {
            End::From => g.edges[at].from = page,
            End::To => g.edges[at].to = page,
        }
        rewritten[at] = true;
    }
    let untouched: HashSet<(NodeId, NodeId, EdgeCategoryId)> = g
        .edges
        .iter()
        .zip(&rewritten)
        .filter(|(_, r)| !**r)
        .map(|(e, _)| (e.from, e.to, e.category))
        .collect();
    let mut seen: HashSet<(NodeId, NodeId, EdgeCategoryId)> = HashSet::new();
    let keep: Vec<bool> = g
        .edges
        .iter()
        .zip(&rewritten)
        .map(|(e, &r)| {
            let key = (e.from, e.to, e.category);
            !r || (!untouched.contains(&key) && seen.insert(key))
        })
        .collect();
    let mut flags = keep.into_iter();
    g.edges.retain(|_| flags.next().unwrap_or(true));
    lifted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_route_path_parses_every_shape() {
        assert_eq!(nav_route_path("page:/x"), Some("/x"));
        assert_eq!(nav_route_path("GET /x"), Some("/x"));
        assert_eq!(nav_route_path("route:/x"), Some("/x"));
        assert_eq!(nav_route_path("page:/x @web"), Some("/x"));
        assert_eq!(nav_route_path("GET /x @services/users"), Some("/x"));
        assert_eq!(nav_route_path("page:/**"), Some("/**"));
        // Not a route path.
        assert_eq!(nav_route_path("src::App"), None);
        assert_eq!(nav_route_path("get /x"), None);
        assert_eq!(nav_route_path("GET x"), None);
        assert_eq!(nav_route_path("endpoint:GET:/x"), None);
    }

    #[test]
    fn catch_alls_split_root_from_scoped() {
        for root in [
            "/**",
            "*",
            "/:pathMatch(.*)*",
            "/[...all]",
            "/[[...all]]",
            "/:rest+",
        ] {
            assert_eq!(classify(root), Shape::Root, "{root}");
        }
        let lit = |s: &str| Seg::Lit(s.to_string());
        assert_eq!(
            classify("/docs/**"),
            Shape::Scoped {
                prefix: vec![lit("docs")],
                optional: true
            }
        );
        assert_eq!(
            classify("/docs/[...slug]"),
            Shape::Scoped {
                prefix: vec![lit("docs")],
                optional: false
            }
        );
        assert_eq!(
            classify("/u/:id/:rest(.*)*"),
            Shape::Scoped {
                prefix: vec![lit("u"), Seg::Param],
                optional: true
            }
        );
        // A named parameter, a mid-path star and `[id]` are not catch-alls.
        assert_eq!(
            classify("/users/:id"),
            Shape::Plain(vec![lit("users"), Seg::Param])
        );
        assert_eq!(
            classify("/a/*/b"),
            Shape::Plain(vec![lit("a"), lit("*"), lit("b")])
        );
        assert_eq!(
            classify("/p/[id]"),
            Shape::Plain(vec![lit("p"), Seg::Param])
        );
    }

    #[test]
    fn link_tail_and_tier_marker_are_stripped() {
        assert_eq!(split_link("/a?x=1#top"), (false, "/a"));
        assert_eq!(split_link("href:/logout"), (true, "/logout"));
        assert_eq!(split_link("href:/a#f"), (true, "/a"));
    }

    fn index(routes: &[(&str, Shape)]) -> NavIndex {
        let mut idx = NavIndex {
            any_nav: true,
            ..NavIndex::default()
        };
        for (i, (norm, shape)) in routes.iter().enumerate() {
            let at = idx.routes.len();
            if let Shape::Plain(segs) = shape {
                if segs.iter().all(|s| matches!(s, Seg::Lit(_))) {
                    idx.exact.entry(norm.to_string()).or_default().push(at);
                }
                idx.by_len.entry(segs.len()).or_default().push(at);
            }
            idx.routes.push(NavRoute {
                id: NodeId(i as u64),
                owner: None,
                shape: shape.clone(),
            });
        }
        idx
    }

    fn look(idx: &NavIndex, link: &str) -> Option<(Tier, Vec<NodeId>)> {
        let norm = normalise_http_path(link);
        idx.lookup(&norm, &segs_of(&norm), None)
    }

    #[test]
    fn tiers_run_in_order_and_the_first_hit_wins() {
        let idx = index(&[
            ("/settings", classify("/settings")),
            ("/users/{}", classify("/users/:id")),
            ("/users/me", classify("/users/me")),
            ("/docs", classify("/docs/[[...slug]]")),
            ("/files", classify("/files/[...path]")),
        ]);
        assert_eq!(
            look(&idx, "/users/me"),
            Some((Tier::Exact, vec![NodeId(2)]))
        );
        assert_eq!(look(&idx, "/users/7"), Some((Tier::Param, vec![NodeId(1)])));
        // A dynamic link segment never binds a literal route.
        assert_eq!(
            look(&idx, "/users/${...}"),
            Some((Tier::Param, vec![NodeId(1)]))
        );
        // An optional catch-all serves its bare prefix; a required one does not.
        assert_eq!(look(&idx, "/docs"), Some((Tier::Scoped, vec![NodeId(3)])));
        assert_eq!(
            look(&idx, "/files/a/b"),
            Some((Tier::Scoped, vec![NodeId(4)]))
        );
        assert_eq!(look(&idx, "/files"), None);
        // A child table mounted under a parent prefix in another file.
        assert_eq!(
            look(&idx, "/admin/settings"),
            Some((Tier::Suffix, vec![NodeId(0)]))
        );
        // A parameter-only route never suffix-binds (no literal compared).
        let bare = index(&[("/{}", classify("/:id"))]);
        assert_eq!(look(&bare, "/a/b"), None);
        assert_eq!(look(&idx, "/nowhere"), None);
    }
}
