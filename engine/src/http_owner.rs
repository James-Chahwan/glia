//! Owner segment on HTTP qnames (LB.4a): ROUTE / ENDPOINT / page nodes inside
//! a nested project root are qualified with ` @<project path>`. LB.8 extends
//! the same rule to the SIDES of every named channel: QUEUE_PRODUCER /
//! QUEUE_CONSUMER, WS_HANDLER / WS_CLIENT, GRAPHQL_RESOLVER /
//! GRAPHQL_OPERATION, GRPC_CLIENT and the A5.3 GRPC_SERVER marker. LB.8b
//! adds the last two channel families: RPC_PROCEDURE / RPC_CALL (tRPC,
//! Connect, Twirp) and EVENT_EMITTER / EVENT_HANDLER.
//!
//! WHY. NodeId hashes (repo, kind, qname), and an HTTP qname is only a method
//! and a path. Two services in ONE repo serving `/health` were therefore ONE
//! `GET /health` node, HANDLED_BY both handlers, and every client of either
//! reached both. The same collapse hit client ENDPOINTs of two apps calling
//! one path, and `page:` nodes of two SPAs. Qualifying with the owning project
//! gives each its own identity: `GET /health @services/users`,
//! `endpoint:GET:/health @web`, `page:/users @web`.
//!
//! CHANNEL SIDES (LB.8). A channel qname is only the channel: two services
//! publishing `orders.created` were ONE `queue_producer:orders.created`, two
//! NestJS apps serving `@Query getUser` ONE `graphql_resolver:getUser`
//! HANDLED_BY both apps' methods, and `glia arch` placed each collapsed node
//! in one project. The CHANNEL (topic, ws path, graphql field, proto service)
//! is genuinely shared and stays the pairing key everywhere, but no node
//! represents it: each of these kinds is ONE SIDE of it in one service, the
//! conjugate tier of ROUTE / ENDPOINT, so it takes the owner
//! (`queue_producer:orders.created @services/orders`). The proto-declared
//! GRPC_SERVICE (`grpc:<pkg>.<Svc>`) is the contract itself, like
//! MESSAGE_TYPE: vendored copies of one `.proto` in two projects are one
//! contract, so it is never owned. This is what a multi-repo build of the
//! same services already gives (each repo its own RepoId); the monorepo now
//! agrees. Within one project the A2.8 rule stands: one producer node per
//! topic however many files publish.
//!
//! RPC AND EVENT SIDES (LB.8b). `rpc:<path>` / `rpc_call:<path>` are network
//! RPC sides like the gRPC ones and pair across owners on the exact path.
//! `event_emit:<name>` / `event_handle:<name>` take the owner too, but the
//! EventBusResolver pairs an IN-PROCESS event only inside one owner (or with
//! an unowned side): two nested projects are two processes. A side the
//! extractor marked transport-scoped (NestJS microservices, EventBridge)
//! pairs across owners like a queue. Only the extractor's prefixed qnames are
//! owned: a Solidity `event BidPlaced(..)` is an EVENT_EMITTER with an
//! ordinary code qname, already path-scoped, and an on-chain event is read
//! off-chain by other processes; it is left alone and counted `declared`.
//!
//! WHO OWNS A NODE. The longest nested project root (A8.4) enclosing the
//! node's file (the `service_of` rule in [`crate::arch`]). The repo root is
//! never an owner, so a single-project repo, and every file under only the
//! root manifest, keeps today's qnames exactly. Platform-host shells (LA.5:
//! Flutter / React Native `android/`, `ios/`, ...) are not owners either;
//! their files fall to the app that owns them. So `@<owner>` always equals
//! the service id `glia arch` prints for a single repo. The owner is
//! structural, never "only when two services collide": a node's identity does
//! not depend on its siblings. The rule is by file, so an SDL `.graphql` file
//! inside a project owns its fields and one outside every project does not.
//!
//! WHERE IT RUNS. Post-cache, LAST among the grafts that mint or re-key an
//! owned kind (the A11.2 endpoint fold, which keys client endpoints by their
//! owner-free path; LA.6d's Next.js pages; LA.4's queue-topic const fold;
//! the A5.2 / A5.3 RPC needles and LA.17's Connect / Twirp RPC_PROCEDURE /
//! RPC_CALL graft), from `build::grafts::apply_post_cache`. The
//! cache keeps the owner-free parse, so adding or removing a manifest never
//! needs a cache flush.
//!
//! PAIRING IGNORES OWNERS. Every resolver strips the segment
//! (`code_domain::endpoint::split_owner`) before it reads a method, path,
//! topic, field or service name, so two owners on one channel pair
//! all-to-all; narrowing an HTTP pairing by project is LB.4b. The one
//! exception is the in-process event (LB.8b, above): the EventBusResolver
//! reads the owner back and keeps an EVENT_FLOWS only inside one owner, with
//! an unowned side, or across a transport.
//!
//! Module slot declared by L0.2 so its owner edits only this file.
//! Crate-private: cross-module items are `pub(crate)`.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};

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

/// The channel family of an owned kind. The Http rows keep LB.4a's routes /
/// endpoints / pages accounting and its `[http-owner]` line; Queue / Ws /
/// Graphql / Grpc are counted on the LB.8 `[channel-owner]` line, and Rpc /
/// Event (LB.8b) on a second `[channel-owner]` line, so LB.8's reads exactly
/// as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mechanism {
    Http,
    Queue,
    Ws,
    Graphql,
    Grpc,
    Rpc,
    Event,
}

/// Every kind the owner pass qualifies, with its mechanism and the qname
/// prefix a node of that kind must carry to be owned (`""` = any). A kind
/// absent here is never owned: GRPC_SERVICE and MESSAGE_TYPE are shared
/// contracts. A node of a listed kind whose qname lacks the prefix (a
/// Solidity-declared event, a code qname) is left alone and counted
/// `declared`.
const OWNED: &[(NodeKindId, Mechanism, &str)] = &[
    (node_kind::ROUTE, Mechanism::Http, ""),
    (node_kind::ENDPOINT, Mechanism::Http, ""),
    (node_kind::QUEUE_PRODUCER, Mechanism::Queue, ""),
    (node_kind::QUEUE_CONSUMER, Mechanism::Queue, ""),
    (node_kind::WS_HANDLER, Mechanism::Ws, ""),
    (node_kind::WS_CLIENT, Mechanism::Ws, ""),
    (node_kind::GRAPHQL_RESOLVER, Mechanism::Graphql, ""),
    (node_kind::GRAPHQL_OPERATION, Mechanism::Graphql, ""),
    (node_kind::GRPC_CLIENT, Mechanism::Grpc, ""),
    (node_kind::GRPC_SERVER, Mechanism::Grpc, ""),
    (node_kind::RPC_PROCEDURE, Mechanism::Rpc, "rpc:"),
    (node_kind::RPC_CALL, Mechanism::Rpc, "rpc_call:"),
    (node_kind::EVENT_EMITTER, Mechanism::Event, "event_emit:"),
    (node_kind::EVENT_HANDLER, Mechanism::Event, "event_handle:"),
];

/// The mechanism and required qname prefix of an owned kind.
fn owned_row(kind: NodeKindId) -> Option<(Mechanism, &'static str)> {
    OWNED.iter().find(|(k, _, _)| *k == kind).map(|(_, m, p)| (*m, *p))
}

/// Which marker bucket a qualified node counts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Route,
    Page,
    Endpoint,
    Queue,
    Ws,
    Graphql,
    Grpc,
    Rpc,
    Event,
}

impl Class {
    /// Which marker line (and which owner / unplaced / foreign tallies) a
    /// qualified node counts on.
    fn family(self) -> Family {
        match self {
            Class::Route | Class::Page | Class::Endpoint => Family::Http,
            Class::Queue | Class::Ws | Class::Graphql | Class::Grpc => Family::Channel,
            Class::Rpc | Class::Event => Family::RpcEvent,
        }
    }
}

/// The three marker lines' families: LB.4a HTTP, LB.8 channel sides, LB.8b
/// RPC and event sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Http,
    Channel,
    RpcEvent,
}

impl Family {
    fn of(mechanism: Mechanism) -> Self {
        match mechanism {
            Mechanism::Http => Family::Http,
            Mechanism::Queue | Mechanism::Ws | Mechanism::Graphql | Mechanism::Grpc => {
                Family::Channel
            }
            Mechanism::Rpc | Mechanism::Event => Family::RpcEvent,
        }
    }
}

/// One planned rekey inside one parse.
struct Move<'o> {
    old: NodeId,
    new: NodeId,
    qname: String,
    class: Class,
    owner: &'o str,
}

/// What the pass did to one repo, for the `[http-owner]` and
/// `[channel-owner]` markers. Node counts are DISTINCT ids, so a path served
/// from two files of one project counts once, a TypeScript endpoint with
/// several call sites counts once, and a topic published from two files of
/// one project counts once.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct OwnerStats {
    /// Qualified server ROUTEs (qname not `page:`).
    pub routes: usize,
    /// Qualified client ENDPOINTs.
    pub endpoints: usize,
    /// Qualified `page:` ROUTEs (client-router pages, LB.4c).
    pub pages: usize,
    /// Owners that qualified at least one HTTP node.
    pub owners: usize,
    /// ROUTE / ENDPOINT nodes with no file on their cells or in their parse:
    /// left alone.
    pub unplaced: usize,
    /// References to a re-keyed HTTP id from a parse that does not hold that
    /// node (a cross-file edge, ref or call). A per-parse rekey cannot see
    /// them, so they would dangle; they are counted so that is visible, never
    /// silent.
    pub foreign: usize,
    /// ROUTE / ENDPOINT nodes examined. Zero means no HTTP surface.
    pub seen: usize,
    /// LB.8: qualified QUEUE_PRODUCER / QUEUE_CONSUMER nodes.
    pub queue: usize,
    /// LB.8: qualified WS_HANDLER / WS_CLIENT nodes.
    pub ws: usize,
    /// LB.8: qualified GRAPHQL_RESOLVER / GRAPHQL_OPERATION nodes.
    pub graphql: usize,
    /// LB.8: qualified GRPC_CLIENT / GRPC_SERVER nodes.
    pub grpc: usize,
    /// LB.8: owners that qualified at least one channel-side node. Kept apart
    /// from `owners` so the `[http-owner]` line reads exactly as before.
    pub chan_owners: usize,
    /// LB.8: channel-side nodes with no file: left alone, so they stay
    /// owner-free and pair with every owner of their channel.
    pub chan_unplaced: usize,
    /// LB.8: `foreign` for re-keyed channel-side ids.
    pub chan_foreign: usize,
    /// LB.8b: qualified RPC_PROCEDURE / RPC_CALL nodes (tRPC, Connect, Twirp).
    pub rpc: usize,
    /// LB.8b: qualified EVENT_EMITTER / EVENT_HANDLER nodes.
    pub event: usize,
    /// LB.8b: RPC / event nodes whose qname lacks the row's prefix (a
    /// Solidity `event X(..)` with a code qname): never owned.
    pub declared: usize,
    /// LB.8b: owners that qualified at least one RPC / event side.
    pub rpc_event_owners: usize,
    /// LB.8b: `chan_unplaced` for RPC / event sides.
    pub rpc_event_unplaced: usize,
    /// LB.8b: `chan_foreign` for re-keyed RPC / event ids.
    pub rpc_event_foreign: usize,
}

impl OwnerStats {
    /// fired_on markers.
    ///
    /// Once per repo that has a nested project root AND an HTTP surface:
    ///   `[http-owner] qualified routes=R endpoints=E pages=P over O owners (unplaced=U foreign=F) repo=<label>`
    ///
    /// Once per repo that has a nested project root and qualified (or failed
    /// to place, or saw a dangling reference to) a channel side:
    ///   `[channel-owner] qualified queue=Q ws=W graphql=G grpc=R over O owners (unplaced=U foreign=F) repo=<label>`
    ///
    /// LB.8b, a SECOND line (LB.8's stays byte-identical), once per repo that
    /// has a nested project root and qualified, declined (`declared`), failed
    /// to place or saw a dangling reference to an RPC or event side:
    ///   `[channel-owner] qualified rpc=P event=E over O owners (declared=D unplaced=U foreign=F) repo=<label>`
    pub(crate) fn report(&self, repo_label: &str) {
        if self.seen > 0 {
            eprintln!(
                "[http-owner] qualified routes={} endpoints={} pages={} over {} owners (unplaced={} foreign={}) repo={repo_label}",
                self.routes, self.endpoints, self.pages, self.owners, self.unplaced, self.foreign
            );
        }
        let qualified = self.queue + self.ws + self.graphql + self.grpc;
        if qualified + self.chan_unplaced + self.chan_foreign > 0 {
            eprintln!(
                "[channel-owner] qualified queue={} ws={} graphql={} grpc={} over {} owners (unplaced={} foreign={}) repo={repo_label}",
                self.queue,
                self.ws,
                self.graphql,
                self.grpc,
                self.chan_owners,
                self.chan_unplaced,
                self.chan_foreign
            );
        }
        let rpc_event = self.rpc
            + self.event
            + self.declared
            + self.rpc_event_unplaced
            + self.rpc_event_foreign;
        if rpc_event > 0 {
            eprintln!(
                "[channel-owner] qualified rpc={} event={} over {} owners (declared={} unplaced={} foreign={}) repo={repo_label}",
                self.rpc,
                self.event,
                self.rpc_event_owners,
                self.declared,
                self.rpc_event_unplaced,
                self.rpc_event_foreign
            );
        }
    }
}

/// Qualify every owned node of one repo ([`OWNED`]: ROUTE / ENDPOINT / page
/// nodes and the channel sides) with its owner. A repo with no owner (no
/// nested root) is returned untouched, with zeroed stats, before anything is
/// read.
///
/// Per parse, in `fp.nodes` order: the node's file is [`node_file`]
/// (POSITION, else the ENDPOINT_HIT / ROUTE_METHOD `file`), falling back to
/// the parse's MODULE file (a Flask ROUTE carries only a bare-verb
/// ROUTE_METHOD). The new id is `NodeId::from_parts` over the qualified qname
/// and [`rekey_node`] moves it with every reference inside the parse. A node
/// already carrying an owner is left alone, so the pass is idempotent.
///
/// `parses` may come in any order (HashMap order at the call site): every
/// parse is re-keyed on its own and the stats are sums over sets, so the
/// result does not depend on it.
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
    let mut census = Census::default();
    let plans: Vec<Vec<Move<'_>>> = parses
        .iter()
        .map(|fp| plan_parse(fp, owners, repo, &mut census))
        .collect();

    // The foreign census, before anything moves. The value says whether the
    // moved id is an HTTP node (`foreign`), a channel side (`chan_foreign`)
    // or an RPC / event side (`rpc_event_foreign`).
    let moved: HashMap<NodeId, Family> =
        plans.iter().flatten().map(|m| (m.old, m.class.family())).collect();
    if !moved.is_empty() {
        for fp in &parses {
            let own: HashSet<NodeId> = fp.nodes.iter().map(|n| n.id).collect();
            let mut dangles = |id: NodeId| {
                if own.contains(&id) {
                    return;
                }
                match moved.get(&id) {
                    Some(Family::Http) => stats.foreign += 1,
                    Some(Family::Channel) => stats.chan_foreign += 1,
                    Some(Family::RpcEvent) => stats.rpc_event_foreign += 1,
                    None => {}
                }
            };
            for e in &fp.edges {
                dangles(e.from);
                dangles(e.to);
            }
            for r in &fp.refs {
                dangles(r.from);
            }
            for c in &fp.calls {
                dangles(c.from);
            }
        }
    }

    let mut qualified: [HashSet<NodeId>; 9] = Default::default();
    let (mut http_owners, mut chan_owners, mut rpc_event_owners): (
        BTreeSet<&str>,
        BTreeSet<&str>,
        BTreeSet<&str>,
    ) = Default::default();
    for (fp, moves) in parses.iter_mut().zip(plans) {
        for m in moves {
            rekey_node(fp, m.old, m.new, &m.qname);
            qualified[m.class as usize].insert(m.new);
            match m.class.family() {
                Family::Http => http_owners.insert(m.owner),
                Family::Channel => chan_owners.insert(m.owner),
                Family::RpcEvent => rpc_event_owners.insert(m.owner),
            };
        }
    }
    let count = |c: Class| qualified[c as usize].len();
    stats.routes = count(Class::Route);
    stats.pages = count(Class::Page);
    stats.endpoints = count(Class::Endpoint);
    stats.queue = count(Class::Queue);
    stats.ws = count(Class::Ws);
    stats.graphql = count(Class::Graphql);
    stats.grpc = count(Class::Grpc);
    stats.rpc = count(Class::Rpc);
    stats.event = count(Class::Event);
    stats.owners = http_owners.len();
    stats.chan_owners = chan_owners.len();
    stats.rpc_event_owners = rpc_event_owners.len();
    stats.unplaced = census.unplaced.len();
    stats.chan_unplaced = census.chan_unplaced.len();
    stats.rpc_event_unplaced = census.rpc_event_unplaced.len();
    stats.declared = census.declared.len();
    stats.seen = census.seen.len();
    stats
}

/// Distinct ids the planning walk examined or could not place.
#[derive(Default)]
struct Census {
    /// HTTP nodes examined (the `[http-owner]` gate).
    seen: HashSet<NodeId>,
    unplaced: HashSet<NodeId>,
    chan_unplaced: HashSet<NodeId>,
    rpc_event_unplaced: HashSet<NodeId>,
    /// LB.8b: RPC / event nodes whose qname lacks their row's prefix.
    declared: HashSet<NodeId>,
}

/// The rekeys one parse needs, in `fp.nodes` order, one per distinct id.
fn plan_parse<'o>(
    fp: &FileParse,
    owners: &'o OwnerIndex,
    repo: RepoId,
    census: &mut Census,
) -> Vec<Move<'o>> {
    let mut moves = Vec::new();
    let mut planned: HashSet<NodeId> = HashSet::new();
    let mut parse_file: Option<Option<String>> = None;
    for node in &fp.nodes {
        let Some(&kind) = fp.nav.kind_by_id.get(&node.id) else {
            continue;
        };
        let Some((mechanism, prefix)) = owned_row(kind) else {
            continue;
        };
        if !planned.insert(node.id) {
            continue;
        }
        let family = Family::of(mechanism);
        if family == Family::Http {
            census.seen.insert(node.id);
        }
        let Some(qname) = fp.nav.qname_by_id.get(&node.id) else {
            continue;
        };
        if !qname.starts_with(prefix) {
            census.declared.insert(node.id);
            continue;
        }
        if split_owner(qname).1.is_some() {
            continue;
        }
        let file = node_file(node)
            .or_else(|| parse_file.get_or_insert_with(|| module_file(fp)).clone());
        let Some(file) = file else {
            match family {
                Family::Http => census.unplaced.insert(node.id),
                Family::Channel => census.chan_unplaced.insert(node.id),
                Family::RpcEvent => census.rpc_event_unplaced.insert(node.id),
            };
            continue;
        };
        let Some(owner) = owners.owner_of(&file) else {
            continue;
        };
        let new_qname = with_owner(qname, &owner_segment(owner));
        let class = match mechanism {
            Mechanism::Http if kind == node_kind::ENDPOINT => Class::Endpoint,
            Mechanism::Http if qname.starts_with("page:") => Class::Page,
            Mechanism::Http => Class::Route,
            Mechanism::Queue => Class::Queue,
            Mechanism::Ws => Class::Ws,
            Mechanism::Graphql => Class::Graphql,
            Mechanism::Grpc => Class::Grpc,
            Mechanism::Rpc => Class::Rpc,
            Mechanism::Event => Class::Event,
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
        fp.edges.push(Edge { from: r, to: h, category: edge_category::HANDLED_BY, confidence: Confidence::Strong, cells: Vec::new() });
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
            OwnerStats {
                routes: 2,
                owners: 2,
                seen: 1,
                ..OwnerStats::default()
            }
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
        other.edges.push(Edge { from: caller, to: ep, category: edge_category::CALLS, confidence: Confidence::Strong, cells: Vec::new() });
        let mut bare = FileParse::default();
        let lost = push(&mut bare, node_kind::ROUTE, "ANY /legacy", "ANY /legacy", vec![]);

        let stats = qualify_repo([&mut fp, &mut other, &mut bare], &idx, REPO);
        assert_eq!(
            stats,
            OwnerStats {
                endpoints: 1,
                pages: 1,
                owners: 1,
                unplaced: 1,
                foreign: 1,
                seen: 3,
                ..OwnerStats::default()
            }
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

    /// LB.8: every channel side is qualified and counted on the channel line;
    /// the shared contracts (GRPC_SERVICE, MESSAGE_TYPE) are never owned; the
    /// HTTP counters (and so the `[http-owner]` line) do not move.
    #[test]
    fn channel_sides_are_owned_and_contracts_stay_shared() {
        let idx = OwnerIndex::from_roots(&[root("services/orders", "python"), root("web", "npm")]);
        let mut fp = FileParse::default();
        let file = "services/orders/app.py";
        push(&mut fp, node_kind::MODULE, "app", "services::orders::app", vec![position(file)]);
        let f = push(&mut fp, node_kind::FUNCTION, "place", "services::orders::app::place", vec![position(file)]);
        let sides = [
            (node_kind::QUEUE_PRODUCER, "queue_producer:orders.created"),
            (node_kind::QUEUE_CONSUMER, "queue_consumer:orders.created"),
            (node_kind::WS_HANDLER, "ws:/ws"),
            (node_kind::WS_CLIENT, "ws_client:/ws"),
            (node_kind::GRAPHQL_RESOLVER, "graphql_resolver:getUser"),
            (node_kind::GRAPHQL_OPERATION, "graphql_op:getUser"),
            (node_kind::GRPC_CLIENT, "grpc_client:UserService"),
            (node_kind::GRPC_SERVER, "grpc_server:UserService"),
        ];
        for (kind, q) in sides {
            let n = push(&mut fp, kind, q, q, vec![position(file)]);
            fp.edges.push(Edge { from: f, to: n, category: edge_category::USES, confidence: Confidence::Strong, cells: Vec::new() });
        }
        let svc = push(&mut fp, node_kind::GRPC_SERVICE, "UserService", "grpc:user.UserService", vec![position(file)]);
        let msg = push(&mut fp, node_kind::MESSAGE_TYPE, "User", "message:proto:user.User", vec![position(file)]);

        let stats = qualify_repo([&mut fp], &idx, REPO);
        assert_eq!(
            stats,
            OwnerStats { queue: 2, ws: 2, graphql: 2, grpc: 2, chan_owners: 1, ..OwnerStats::default() }
        );
        for (kind, q) in sides {
            let new = id(kind, &format!("{q} @services/orders"));
            assert!(fp.nodes.iter().any(|n| n.id == new), "{q} qualified");
            assert!(
                fp.edges.iter().any(|e| e.from == f && e.to == new),
                "{q}: the USES edge follows the rekey"
            );
            assert_eq!(fp.nav.name_by_id.get(&new).map(String::as_str), Some(q), "display name kept");
        }
        assert!(fp.nodes.iter().any(|n| n.id == svc), "GRPC_SERVICE is a shared contract");
        assert!(fp.nodes.iter().any(|n| n.id == msg), "MESSAGE_TYPE is a shared contract");
    }

    /// A channel side with no file is counted and left owner-free; a reference
    /// to a re-keyed side from another parse is counted as a channel foreign,
    /// never as an HTTP one.
    #[test]
    fn channel_unplaced_and_foreign_are_counted_apart_from_http() {
        let idx = OwnerIndex::from_roots(&[root("web", "npm")]);
        let mut fp = FileParse::default();
        let client = push(&mut fp, node_kind::WS_CLIENT, "/ws", "ws_client:/ws", vec![position("web/chat.ts")]);
        let mut other = FileParse::default();
        let caller = push(&mut other, node_kind::FUNCTION, "f", "lib::f", vec![]);
        other.edges.push(Edge { from: caller, to: client, category: edge_category::USES, confidence: Confidence::Strong, cells: Vec::new() });
        let mut bare = FileParse::default();
        let lost = push(&mut bare, node_kind::QUEUE_PRODUCER, "t", "queue_producer:t", vec![]);

        let stats = qualify_repo([&mut fp, &mut other, &mut bare], &idx, REPO);
        assert_eq!(
            stats,
            OwnerStats { ws: 1, chan_owners: 1, chan_unplaced: 1, chan_foreign: 1, ..OwnerStats::default() }
        );
        assert_eq!(bare.nodes[0].id, lost);
        assert_eq!(fp.nodes[0].id, id(node_kind::WS_CLIENT, "ws_client:/ws @web"));
    }

    /// LB.8b: RPC and event sides are owned and counted apart from LB.8's
    /// channel sides (whose counters, and so whose line, do not move); a
    /// Solidity-shaped code qname is left alone and counted `declared`; every
    /// cell rides the rekey, so an extractor's transport ORIGIN survives.
    #[test]
    fn rpc_and_event_sides_are_owned_and_code_qname_events_are_declared() {
        let idx = OwnerIndex::from_roots(&[root("services/users", "npm"), root("contracts", "npm")]);
        let mut fp = FileParse::default();
        let file = "services/users/router.ts";
        push(&mut fp, node_kind::MODULE, "router", "services::users::router", vec![position(file)]);
        let transport = Cell {
            kind: cell_type::ORIGIN,
            payload: CellPayload::Json(
                r#"{"provenance":"synthetic","delivery":"transport","via":"nestjs-microservices"}"#.into(),
            ),
        };
        let sides = [
            (node_kind::RPC_PROCEDURE, "rpc:user.list"),
            (node_kind::RPC_CALL, "rpc_call:user.list"),
            (node_kind::EVENT_EMITTER, "event_emit:orderPlaced"),
            (node_kind::EVENT_HANDLER, "event_handle:orderShipped"),
        ];
        for (kind, q) in sides {
            // No POSITION on the side itself: the MODULE file places it.
            push(&mut fp, kind, q, q, vec![transport.clone()]);
        }
        let mut sol = FileParse::default();
        let bid_q = "contracts::Auction::Auction::BidPlaced";
        let bid = push(&mut sol, node_kind::EVENT_EMITTER, "BidPlaced", bid_q, vec![position("contracts/Auction.sol")]);

        let stats = qualify_repo([&mut fp, &mut sol], &idx, REPO);
        assert_eq!(
            stats,
            OwnerStats { rpc: 2, event: 2, rpc_event_owners: 1, declared: 1, ..OwnerStats::default() }
        );
        for (kind, q) in sides {
            let new = id(kind, &format!("{q} @services/users"));
            let node = fp.nodes.iter().find(|n| n.id == new);
            assert!(node.is_some(), "{q} qualified");
            assert_eq!(node.map(|n| n.cells.clone()), Some(vec![transport.clone()]), "{q}: cells ride the rekey");
            assert_eq!(fp.nav.name_by_id.get(&new).map(String::as_str), Some(q), "display name kept");
        }
        assert_eq!(sol.nodes[0].id, bid, "a code-qname event is never owned");
        assert_eq!(sol.nav.qname_by_id.get(&bid).map(String::as_str), Some(bid_q));
    }

    /// An RPC / event side with no file is counted on the LB.8b tallies, and a
    /// reference to a re-keyed one from another parse is an LB.8b foreign,
    /// never an LB.8 one.
    #[test]
    fn rpc_event_unplaced_and_foreign_are_counted_apart_from_channels() {
        let idx = OwnerIndex::from_roots(&[root("web", "npm")]);
        let mut fp = FileParse::default();
        let call = push(&mut fp, node_kind::RPC_CALL, "user.list", "rpc_call:user.list", vec![position("web/users.tsx")]);
        let mut other = FileParse::default();
        let caller = push(&mut other, node_kind::FUNCTION, "f", "lib::f", vec![]);
        other.edges.push(Edge { from: caller, to: call, category: edge_category::USES, confidence: Confidence::Strong, cells: Vec::new() });
        let mut bare = FileParse::default();
        let lost = push(&mut bare, node_kind::EVENT_HANDLER, "x", "event_handle:x", vec![]);

        let stats = qualify_repo([&mut fp, &mut other, &mut bare], &idx, REPO);
        assert_eq!(
            stats,
            OwnerStats {
                rpc: 1,
                rpc_event_owners: 1,
                rpc_event_unplaced: 1,
                rpc_event_foreign: 1,
                ..OwnerStats::default()
            }
        );
        assert_eq!(bare.nodes[0].id, lost);
        assert_eq!(fp.nodes[0].id, id(node_kind::RPC_CALL, "rpc_call:user.list @web"));
    }
}
