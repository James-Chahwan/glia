//! Owner segment on HTTP qnames (LB.4a): ROUTE / ENDPOINT / page nodes inside
//! a nested project root are qualified with ` @<project path>`.
//!
//! WHY. NodeId hashes (repo, kind, qname), and an HTTP qname is only a method
//! and a path. Two services in ONE repo serving `/health` were therefore ONE
//! `GET /health` node, HANDLED_BY both handlers, and every client of either
//! reached both. The same collapse hit client ENDPOINTs of two apps calling
//! one path, and `page:` nodes of two SPAs. Qualifying with the owning project
//! gives each its own identity: `GET /health @services/users`,
//! `endpoint:GET:/health @web`, `page:/users @web`.
//!
//! WHO OWNS A NODE. The longest nested project root (A8.4) enclosing the
//! node's file (the `service_of` rule in [`crate::arch`]). The repo root is
//! never an owner, so a single-project repo, and every file under only the
//! root manifest, keeps today's qnames exactly. Platform-host shells (LA.5:
//! Flutter / React Native `android/`, `ios/`, ...) are not owners either;
//! their files fall to the app that owns them. So `@<owner>` always equals
//! the service id `glia arch` prints for a single repo. The owner is
//! structural, never "only when two services collide": a node's identity does
//! not depend on its siblings.
//!
//! WHERE IT RUNS. Post-cache, right after the A11.2 endpoint fold (which
//! re-keys client endpoints by path and must see owner-free qnames), from
//! `build::grafts::apply_post_cache`. The cache keeps the owner-free parse,
//! so adding or removing a manifest never needs a cache flush.
//!
//! PAIRING IGNORES OWNERS. The HTTP resolver strips the segment
//! (`code_domain::endpoint::split_owner`) before it reads a method or path,
//! so the `[http]` pairing counts are unchanged; narrowing a pairing by
//! project is LB.4b.
//!
//! Module slot declared by L0.2 so its owner edits only this file.
//! Crate-private: cross-module items are `pub(crate)`.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashSet};

use repo_graph_code_domain::endpoint::{split_owner, with_owner};
use repo_graph_code_domain::project_roots::ProjectRoot;
use repo_graph_code_domain::{FileParse, GRAPH_TYPE, node_kind};
use repo_graph_core::{NodeId, NodeKindId, RepoId};

use crate::arch::node_file;
use crate::rekey::rekey_node;

/// The owner vocabulary of one repo: the repo-relative dirs of its nested
/// project roots, minus platform-host shells, sorted.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct OwnerIndex(Vec<String>);

impl OwnerIndex {
    /// Owners from the walk's roots. The repo root (`""`) is dropped, and so
    /// is every shell `arch::platform_host_owners` folds into its app.
    pub(crate) fn from_roots(roots: &[ProjectRoot]) -> Self {
        let pairs: Vec<(&str, &str)> = roots
            .iter()
            .map(|r| (if r.rel_path.is_empty() { "." } else { r.rel_path.as_str() }, r.ecosystem))
            .collect();
        let hosts = crate::arch::platform_host_owners(&pairs);
        let mut owners: Vec<String> = roots
            .iter()
            .map(|r| r.rel_path.as_str())
            .filter(|p| !p.is_empty() && *p != "." && !hosts.contains_key(*p))
            .map(String::from)
            .collect();
        owners.sort_unstable();
        owners.dedup();
        Self(owners)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The longest owner enclosing `file` (segment-bounded: `webx/a.ts` is not
    /// under `web`), or `None` for a file under no nested root.
    fn owner_of(&self, file: &str) -> Option<&str> {
        self.0
            .iter()
            .filter(|r| {
                file == r.as_str()
                    || file.strip_prefix(r.as_str()).is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
            .map(String::as_str)
    }
}

/// The owner as it is written into a qname. A root path is used verbatim
/// unless it holds whitespace (or a `%`): `split_owner` rejects a tail with
/// whitespace, so those bytes are percent-escaped (`my app` -> `my%20app`)
/// and the segment still round-trips. Every ordinary path is unchanged.
fn owner_segment(rel: &str) -> Cow<'_, str> {
    if !rel.chars().any(|c| c.is_whitespace() || c == '%') {
        return Cow::Borrowed(rel);
    }
    let mut out = String::with_capacity(rel.len() + 8);
    for c in rel.chars() {
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

/// Which marker bucket a qualified node counts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HttpClass {
    Route,
    Page,
    Endpoint,
}

/// One planned rekey inside one parse.
struct Move<'o> {
    old: NodeId,
    new: NodeId,
    qname: String,
    class: HttpClass,
    owner: &'o str,
}

/// What the pass did to one repo, for the `[http-owner]` marker. Node counts
/// are DISTINCT ids, so a path served from two files of one project counts
/// once, and a TypeScript endpoint with several call sites counts once.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct OwnerStats {
    /// Qualified server ROUTEs (qname not `page:`).
    pub routes: usize,
    /// Qualified client ENDPOINTs.
    pub endpoints: usize,
    /// Qualified `page:` ROUTEs (client-router pages, LB.4c).
    pub pages: usize,
    /// Owners that qualified at least one node.
    pub owners: usize,
    /// ROUTE / ENDPOINT nodes with no file on their cells or in their parse:
    /// left alone.
    pub unplaced: usize,
    /// References to a re-keyed id from a parse that does not hold that node
    /// (a cross-file edge, ref or call). A per-parse rekey cannot see them, so
    /// they would dangle; they are counted so that is visible, never silent.
    pub foreign: usize,
    /// ROUTE / ENDPOINT nodes examined. Zero means no HTTP surface.
    pub seen: usize,
}

impl OwnerStats {
    /// fired_on marker, once per repo that has a nested project root AND an
    /// HTTP surface:
    ///   `[http-owner] qualified routes=R endpoints=E pages=P over O owners (unplaced=U foreign=F) repo=<label>`
    pub(crate) fn report(&self, repo_label: &str) {
        if self.seen == 0 {
            return;
        }
        eprintln!(
            "[http-owner] qualified routes={} endpoints={} pages={} over {} owners (unplaced={} foreign={}) repo={repo_label}",
            self.routes, self.endpoints, self.pages, self.owners, self.unplaced, self.foreign
        );
    }
}

/// Qualify every ROUTE / ENDPOINT / page node of one repo with its owner.
/// A repo with no owner (no nested root) is returned untouched, with zeroed
/// stats, before anything is read.
///
/// Per parse, in `fp.nodes` order: the node's file is [`node_file`]
/// (POSITION, else the ENDPOINT_HIT / ROUTE_METHOD `file`), falling back to
/// the parse's MODULE file (a Flask ROUTE carries only a bare-verb
/// ROUTE_METHOD). The new id is `NodeId::from_parts` over the qualified qname
/// and [`rekey_node`] moves it with every reference inside the parse. A node
/// already carrying an owner is left alone, so the pass is idempotent.
pub(crate) fn qualify_repo<'a>(
    parses: impl IntoIterator<Item = &'a mut FileParse>,
    owners: &OwnerIndex,
    repo: RepoId,
) -> OwnerStats {
    let mut stats = OwnerStats::default();
    if owners.is_empty() {
        return stats;
    }
    let mut parses: Vec<&mut FileParse> = parses.into_iter().collect();
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut unplaced: HashSet<NodeId> = HashSet::new();
    let plans: Vec<Vec<Move<'_>>> = parses
        .iter()
        .map(|fp| plan_parse(fp, owners, repo, &mut seen, &mut unplaced))
        .collect();

    // The foreign census, before anything moves.
    let moved: HashSet<NodeId> = plans.iter().flatten().map(|m| m.old).collect();
    if !moved.is_empty() {
        for fp in &parses {
            let own: HashSet<NodeId> = fp.nodes.iter().map(|n| n.id).collect();
            let dangles = |id: NodeId| usize::from(moved.contains(&id) && !own.contains(&id));
            stats.foreign += fp.edges.iter().map(|e| dangles(e.from) + dangles(e.to)).sum::<usize>();
            stats.foreign += fp.refs.iter().map(|r| dangles(r.from)).sum::<usize>();
            stats.foreign += fp.calls.iter().map(|c| dangles(c.from)).sum::<usize>();
        }
    }

    let (mut routes, mut pages, mut endpoints) = (HashSet::new(), HashSet::new(), HashSet::new());
    let mut used: BTreeSet<&str> = BTreeSet::new();
    for (fp, moves) in parses.iter_mut().zip(plans) {
        for m in moves {
            rekey_node(fp, m.old, m.new, &m.qname);
            match m.class {
                HttpClass::Route => routes.insert(m.new),
                HttpClass::Page => pages.insert(m.new),
                HttpClass::Endpoint => endpoints.insert(m.new),
            };
            used.insert(m.owner);
        }
    }
    stats.routes = routes.len();
    stats.pages = pages.len();
    stats.endpoints = endpoints.len();
    stats.owners = used.len();
    stats.unplaced = unplaced.len();
    stats.seen = seen.len();
    stats
}

/// The rekeys one parse needs, in `fp.nodes` order, one per distinct id.
fn plan_parse<'o>(
    fp: &FileParse,
    owners: &'o OwnerIndex,
    repo: RepoId,
    seen: &mut HashSet<NodeId>,
    unplaced: &mut HashSet<NodeId>,
) -> Vec<Move<'o>> {
    let mut moves = Vec::new();
    let mut planned: HashSet<NodeId> = HashSet::new();
    let mut parse_file: Option<Option<String>> = None;
    for node in &fp.nodes {
        let Some(&kind) = fp.nav.kind_by_id.get(&node.id) else {
            continue;
        };
        if !is_http_kind(kind) || !planned.insert(node.id) {
            continue;
        }
        seen.insert(node.id);
        let Some(qname) = fp.nav.qname_by_id.get(&node.id) else {
            continue;
        };
        if split_owner(qname).1.is_some() {
            continue;
        }
        let file = node_file(node)
            .or_else(|| parse_file.get_or_insert_with(|| module_file(fp)).clone());
        let Some(file) = file else {
            unplaced.insert(node.id);
            continue;
        };
        let Some(owner) = owners.owner_of(&file) else {
            continue;
        };
        let new_qname = with_owner(qname, &owner_segment(owner));
        let class = if kind == node_kind::ENDPOINT {
            HttpClass::Endpoint
        } else if qname.starts_with("page:") {
            HttpClass::Page
        } else {
            HttpClass::Route
        };
        moves.push(Move {
            old: node.id,
            new: NodeId::from_parts(GRAPH_TYPE, repo, kind, &new_qname),
            qname: new_qname,
            class,
            owner,
        });
    }
    moves
}

fn is_http_kind(kind: NodeKindId) -> bool {
    kind == node_kind::ROUTE || kind == node_kind::ENDPOINT
}

/// The file of the parse's first MODULE node that names one.
fn module_file(fp: &FileParse) -> Option<String> {
    fp.nodes
        .iter()
        .filter(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::MODULE))
        .find_map(node_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use repo_graph_code_domain::{cell_type, edge_category};
    use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node};

    const REPO: RepoId = RepoId(5);

    fn root(rel: &str, eco: &'static str) -> ProjectRoot {
        ProjectRoot::new(rel.into(), eco, "manifest", None)
    }

    fn id(kind: NodeKindId, q: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, REPO, kind, q)
    }

    fn position(file: &str) -> Cell {
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(r#"{{"file":"{file}","start_line":0,"end_line":1}}"#)),
        }
    }

    fn push(fp: &mut FileParse, kind: NodeKindId, name: &str, q: &str, cells: Vec<Cell>) -> NodeId {
        let n = id(kind, q);
        fp.nodes.push(Node { id: n, repo: REPO, confidence: Confidence::Strong, cells });
        fp.nav.record(n, name, q, kind, None);
        n
    }

    /// A flask-shaped parse: a MODULE with a POSITION, a handler, and a ROUTE
    /// whose only cell is a bare-verb ROUTE_METHOD, HANDLED_BY the handler.
    fn flask(file: &str, handler: &str) -> (FileParse, NodeId, NodeId) {
        let mut fp = FileParse::default();
        let module = file.trim_end_matches(".py").replace('/', "::");
        push(&mut fp, node_kind::MODULE, "app", &module, vec![position(file)]);
        let h = push(&mut fp, node_kind::FUNCTION, handler, &format!("{module}::{handler}"), vec![position(file)]);
        let verb = Cell { kind: cell_type::ROUTE_METHOD, payload: CellPayload::Text("GET".into()) };
        let r = push(&mut fp, node_kind::ROUTE, "GET /health", "GET /health", vec![verb]);
        fp.edges.push(Edge { from: r, to: h, category: edge_category::HANDLED_BY, confidence: Confidence::Strong });
        (fp, r, h)
    }

    #[test]
    fn owner_of_takes_the_longest_segment_bounded_root_and_skips_the_repo_root() {
        let idx = OwnerIndex::from_roots(&[
            root("", "npm"),
            root("services", "npm"),
            root("services/api", "go"),
            root("web", "npm"),
        ]);
        assert_eq!(idx.0, ["services", "services/api", "web"], "the repo root is never an owner");
        assert_eq!(idx.owner_of("services/api/main.go"), Some("services/api"));
        assert_eq!(idx.owner_of("services/other/x.go"), Some("services"));
        assert_eq!(idx.owner_of("web"), Some("web"));
        assert_eq!(idx.owner_of("webx/a.ts"), None, "segment boundary, not a string prefix");
        assert_eq!(idx.owner_of("main.go"), None);
    }

    #[test]
    fn platform_host_shells_are_not_owners() {
        let idx = OwnerIndex::from_roots(&[
            root("mobile", "dart"),
            root("mobile/android", "gradle"),
            root("mobile/android/app", "gradle"),
            root("server", "go"),
        ]);
        assert_eq!(idx.0, ["mobile", "server"]);
        assert_eq!(idx.owner_of("mobile/android/app/src/Main.kt"), Some("mobile"));
        assert!(OwnerIndex::from_roots(&[root("", "go")]).is_empty());
    }

    #[test]
    fn owner_segment_escapes_only_what_split_owner_rejects() {
        assert_eq!(owner_segment("services/users"), "services/users");
        assert_eq!(owner_segment("packages/@shop/web"), "packages/@shop/web");
        assert_eq!(owner_segment("my app/web"), "my%20app/web");
        assert_eq!(owner_segment("a%b"), "a%25b");
        let q = with_owner("GET /x", &owner_segment("my app/web"));
        assert_eq!(split_owner(&q), ("GET /x", Some("my%20app/web")));
    }

    /// Two services serving one path become two ROUTE ids, each still
    /// HANDLED_BY its own handler; a root-level route is untouched.
    #[test]
    fn nested_routes_split_and_root_routes_stay() {
        let idx = OwnerIndex::from_roots(&[root("services/admin", "python"), root("services/users", "python")]);
        let (mut admin, old, admin_h) = flask("services/admin/app.py", "admin_health");
        let (mut users, _, users_h) = flask("services/users/app.py", "users_health");
        let (mut top, top_route, _) = flask("app.py", "top_health");
        let stats = qualify_repo([&mut admin, &mut users, &mut top], &idx, REPO);

        let admin_r = id(node_kind::ROUTE, "GET /health @services/admin");
        let users_r = id(node_kind::ROUTE, "GET /health @services/users");
        assert_eq!(admin.edges[0].from, admin_r);
        assert_eq!(admin.edges[0].to, admin_h);
        assert_eq!(users.edges[0].from, users_r);
        assert_eq!(users.edges[0].to, users_h);
        assert_eq!(admin.nav.name_by_id.get(&admin_r).map(String::as_str), Some("GET /health"));
        assert_eq!(top.edges[0].from, top_route, "outside every nested root: unchanged");
        assert_eq!(top_route, old, "the root-level route keeps the pre-owner id");
        assert_eq!(
            stats,
            OwnerStats { routes: 2, endpoints: 0, pages: 0, owners: 2, unplaced: 0, foreign: 0, seen: 1 }
        );
    }

    /// A reference to a moved id from a parse that does not hold it is
    /// counted; an unplaceable node is counted and left alone; page and
    /// endpoint nodes go to their own buckets.
    #[test]
    fn foreign_unplaced_pages_and_endpoints_are_counted() {
        let idx = OwnerIndex::from_roots(&[root("web", "npm")]);
        let mut fp = FileParse::default();
        push(&mut fp, node_kind::MODULE, "api", "web::api", vec![position("web/api.ts")]);
        let hit = Cell {
            kind: cell_type::ENDPOINT_HIT,
            payload: CellPayload::Json(r#"{"method":"GET","path":"/x","file":"web/api.ts","line":3}"#.into()),
        };
        let ep = push(&mut fp, node_kind::ENDPOINT, "GET /x", "endpoint:GET:/x", vec![hit.clone()]);
        fp.nodes.push(Node { id: ep, repo: REPO, confidence: Confidence::Strong, cells: vec![hit] });
        push(&mut fp, node_kind::ROUTE, "/users", "page:/users", vec![]);
        let mut other = FileParse::default();
        let caller = push(&mut other, node_kind::FUNCTION, "f", "lib::f", vec![]);
        other.edges.push(Edge { from: caller, to: ep, category: edge_category::CALLS, confidence: Confidence::Strong });
        let mut bare = FileParse::default();
        let lost = push(&mut bare, node_kind::ROUTE, "ANY /legacy", "ANY /legacy", vec![]);

        let stats = qualify_repo([&mut fp, &mut other, &mut bare], &idx, REPO);
        assert_eq!(
            stats,
            OwnerStats { routes: 0, endpoints: 1, pages: 1, owners: 1, unplaced: 1, foreign: 1, seen: 3 }
        );
        let new_ep = id(node_kind::ENDPOINT, "endpoint:GET:/x @web");
        assert_eq!(fp.nodes.iter().filter(|n| n.id == new_ep).count(), 2, "both call-site entries move");
        assert!(fp.nav.qname_by_id.values().any(|q| q == "page:/users @web"));
        assert_eq!(bare.nodes[0].id, lost);

        // Idempotent: a second run finds every node already qualified.
        let again = qualify_repo([&mut fp], &idx, REPO);
        assert_eq!((again.routes, again.endpoints, again.pages, again.seen), (0, 0, 0, 2));
    }

    #[test]
    fn a_repo_without_nested_roots_is_untouched() {
        let (mut fp, route, _) = flask("services/admin/app.py", "admin_health");
        let stats = qualify_repo([&mut fp], &OwnerIndex::from_roots(&[root("", "python")]), REPO);
        assert_eq!(stats, OwnerStats::default());
        assert_eq!(fp.edges[0].from, route);
    }
}
