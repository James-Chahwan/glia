//! Inferred-model breaks against a git rev (DT.1o): every client <-> server
//! model exchange the graph pairs, judged at the rev and in the working tree.
//! [`model_breaks`] answers from a built [`RevDelta`]; DT.1p wires the same
//! body ([`model_breaks_of`]) into contract-breaks and review.
//!
//! EXCHANGES. One per side of the rev: every `http_model` pair DT.1m's
//! `glia_graph::wire_pairs` compares (a call site against a handler of a
//! route it is paired with, per direction with a side on both ends) and
//! every `graphql_selection` pair DT.1n's `glia_graph::graphql_pairs`
//! compares (an operation's selection under one root field against the
//! schema type the field returns). Each keeps its ident (pairing, direction,
//! route or op, handler or resolver, endpoint or op, hit file), its client
//! root (the client model, else the ENDPOINT / op), its server root (the
//! server model, else the handler / the schema MESSAGE_TYPE), both sides
//! flattened (`glia_graph::wire_fields` / `graphql_fields_all`; a union
//! client root as the member its server value meets,
//! `ResolvedShape::judged_against`, DT.1p), the static
//! breaking changes DT.1m / DT.1n found, and its sites (the ENDPOINT_HIT's
//! file and 1-based line and the handler site's line; the op's and the root
//! field's locations). Hits that share an ident keep the first by line. An
//! exchange is unjudged (counted, never listed) when its server side is
//! ambiguous, its GraphQL pair was refused or noted open, or a side is open
//! at its root.
//!
//! PAIRING ACROSS THE REV. Before-side ids map through the delta's moves (the
//! rev is built under the working tree's identity, so an unchanged qname is
//! one NodeId). An exchange pairs first on its ident plus its client root,
//! then, within one (pairing, direction, route or op key), the one unmatched
//! candidate left on each side (a renamed client model or handler, a moved
//! hit file). An after-only exchange is a new pairing: it lists only its
//! static breaking changes. A before-only one is lost: counted, never listed
//! (a lost pairing whose channel edge went away is contract-breaks' orphaned
//! client).
//!
//! THE RULES ([`judge`]). C0 / C1 are the client's field at a path at the
//! rev / in the working tree, S0 / S1 the server's. A server change is
//! judged against what the client reads or sends NOW; a client change
//! against the server it meets. Response (names exact): a field renamed
//! under the same member is `json_name_changed` (breaking when the client
//! reads the old name and not the new), else `response_field_removed`
//! (breaking when read), `response_field_added`, `response_type_changed` and
//! `response_field_now_optional` (listed when read); a client's new field the
//! server does not send is `json_field_missing` / `json_field_optional_absent`,
//! a kind it now reads that the server does not send `json_type_mismatch`, a
//! read made required that the server may omit `json_field_may_be_absent`.
//! Request (a Go binder folds names, exact first): `request_json_name_changed`,
//! `request_field_removed`, `new_required_request_field`, `request_field_added`,
//! `request_type_changed`; a client that stops sending a field the binder
//! requires `json_required_missing`, one it binds optionally `json_field_unset`,
//! a new field nobody binds `json_field_dropped`, a kind change it now
//! mismatches `json_type_mismatch`. Selection (GraphQL; an unselected schema
//! field is never a change): `schema_field_removed`, `schema_field_type_changed`,
//! `schema_field_now_nullable`; a new selection the type lacks
//! `graphql_field_undefined`. When both sides moved at a path the server's
//! rule applies (side `both`). Only the topmost of a removed, added or retyped
//! subtree is listed, and no claim that needs an absence is made below a
//! side's open path (row note `open`).
//!
//! TIER AND GATE. An exchange's tier is, per joining category, the strongest
//! `why::tier_of` of the edges joining its ends (HTTP: HTTP_CALLS endpoint ->
//! route, HANDLED_BY route -> handler and, when the server root is a model,
//! the handler's USES of it under the direction's `json_response` /
//! `json_bind` rule; GraphQL: GRAPHQL_CALLS op -> resolver), then the weakest
//! across the categories; a category with no joining edge counts as derived.
//! A row takes its strongest exchange's tier and blocks when it breaks and
//! `check::is_blocking(tier, min_tier)` holds; the default min tier is
//! `fact`, so an inferred (derived) pairing is advisory unless the caller
//! opts in with `derived`.
//!
//! ROWS. Exchanges group by (pairing, direction, client root, server root)
//! in the working tree's ids, sorted by (key, client site); a row's changes
//! are the union over them, deduplicated by (field, rule, before, after),
//! sorted by (field, rule), each with its client and server field lines
//! ([`member_site`], [`selection_site`]). A row with no change is not listed.
//! Rows sort breaking first, then by pairing, direction, client and server
//! qname.
//!
//! fired_on, once per [`model_breaks_of`] call:
//! `[model-breaks] base=<rev> min_tier=<fact|derived> exchanges=<E> http=<H>
//! graphql=<G> rows=<R> breaking=<B> blocking=<K> advisory=<A> compatible=<C>
//! new_pairings=<N> lost=<L> unjudged=<U>` (E = H + G working-tree exchanges,
//! U of them unjudged, B = K + A).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use glia_activation::algo::delta::GraphDelta;
use glia_code_domain::endpoint::split_owner;
use glia_code_domain::evidence::Evidence;
use glia_code_domain::wire::{ModelSchema, WireFormat};
use glia_code_domain::{cell_type, edge_category};
use glia_core::{CellPayload, Edge, EdgeCategoryId, Node, NodeId};
use glia_graph::{
    ClientWire, MergedGraph, WireDirection, WireField, WireFields, client_wires,
    graphql_fields_all, graphql_pairs, wire_fields, wire_pairs,
};

use crate::absence::{self, Absence};
use crate::answers::Locator;
use crate::build::GenerateResult;
use crate::check;
use crate::contract_fields::FieldSide;
use crate::delta::RevDelta;
use crate::why::tier_of;

/// The accepted [`ModelBreakArgs::min_tier`] values, default first.
pub const MODEL_MIN_TIERS: &[&str] = &[check::FACT, check::DERIVED];

/// How [`model_breaks`] gates. Built outside the crate from `Default` plus
/// field assignment.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct ModelBreakArgs {
    /// The weakest tier whose breaking rows block ([`MODEL_MIN_TIERS`]):
    /// `fact` (the default: an inferred pairing is advisory) or `derived`.
    /// Any spelling other than `derived` is judged as `fact`; the surfaces
    /// validate it.
    pub min_tier: &'static str,
}

impl Default for ModelBreakArgs {
    fn default() -> Self {
        ModelBreakArgs {
            min_tier: MODEL_MIN_TIERS[0],
        }
    }
}

/// The inferred-model breaks of the working tree against `base` (module docs).
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ModelBreaks {
    /// The rev as given.
    pub base: String,
    /// The gate the rows were judged under: `fact` | `derived`.
    pub min_tier: &'static str,
    pub rows: Vec<ModelBreak>,
    /// Working-tree exchanges (HTTP + GraphQL).
    pub exchanges: usize,
    /// Of `exchanges`: judged (neither side open at its root nor ambiguous,
    /// and so was the rev's exchange it pairs with).
    pub judged: usize,
    /// Rows with a breaking change: `blocking` + `advisory`.
    pub breaking: usize,
    pub blocking: usize,
    pub advisory: usize,
    pub compatible: usize,
    /// `Some` from [`model_breaks`] exactly when `rows` is empty; always
    /// `None` from [`model_breaks_of`].
    pub absence: Option<Absence>,
    /// The fired_on line this answer printed ([`ModelBreaks::marker`]).
    #[serde(skip)]
    marker: String,
}

impl ModelBreaks {
    /// The `[model-breaks] base=..` line printed once when this answer was
    /// judged (module docs).
    pub fn marker(&self) -> &str {
        &self.marker
    }
}

/// One client model <-> server model pair whose exchange changed.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct ModelBreak {
    /// `http_model` | `graphql_selection`.
    pub pairing: &'static str,
    /// `response` | `request` | `selection`.
    pub direction: &'static str,
    /// The client model (an inline client value: the ENDPOINT; a GraphQL
    /// side: the operation), located in the working tree.
    pub client: FieldSide,
    /// The server model (an inline value: the handler; GraphQL: the schema
    /// type, else the root field), located in the working tree.
    pub server: FieldSide,
    /// Every judged exchange joining the pair, sorted by (key, client site).
    pub exchanges: Vec<ModelExchange>,
    /// `breaking` | `compatible`.
    pub status: &'static str,
    /// `modified` | `added` (every exchange is a new pairing).
    pub change: &'static str,
    /// `server` | `client` | `both` | `pairing` (a new pairing).
    pub changed: &'static str,
    /// The strongest exchange's tier: `fact` | `derived` | `heuristic`.
    pub tier: &'static str,
    /// Breaking and `check::is_blocking(tier, min_tier)`.
    pub blocking: bool,
    /// `new_pairing`, or `open` (a claim needing an absence was withheld).
    pub note: Option<&'static str>,
    /// Sorted by (field, rule).
    pub changes: Vec<ModelChange>,
}

/// One exchange of a row.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ModelExchange {
    /// HTTP: the route qname without its owner (`GET /api/orders/:id`);
    /// GraphQL: `<op type> <op name> <root field>`.
    pub key: String,
    pub tier: &'static str,
    /// The call site (an op's location).
    pub client_site: ModelSite,
    /// The handler's write or bind site (a root field's location).
    pub server_site: ModelSite,
}

/// One changed field of a row.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ModelChange {
    /// The JSON path (`customerName`, `lines[].productId`; `(root)`); a
    /// GraphQL schema-field path. A rename's old path.
    pub field: String,
    /// `removed` | `renamed` | `added` | `type` | `required`.
    pub change: &'static str,
    /// `server` | `client` | `both`.
    pub side: &'static str,
    pub rule: &'static str,
    pub breaking: bool,
    /// The moved side's declaration before: its type text, a rename's JSON
    /// name, `required` / `optional` (`non-null` / `nullable`).
    pub before: Option<String>,
    pub after: Option<String>,
    /// Where the client declares or selects the field.
    pub client: Option<ModelSite>,
    /// Where the server declares the field (GraphQL: the schema type).
    pub server: Option<ModelSite>,
}

/// A located site.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ModelSite {
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// Read from the rev's graph (the field exists only there).
    pub at_base: bool,
}

const HTTP_MODEL: &str = "http_model";
const GRAPHQL_SELECTION: &str = "graphql_selection";
const PRIMITIVE: &str = "model_breaks";
const NOTE_NEW_PAIRING: &str = "new_pairing";
const NOTE_OPEN: &str = "open";
/// `WirePair.notes` / `GraphqlPair.notes` (graph/src/resolvers/models.rs).
const PAIR_AMBIGUOUS: &str = "ambiguous_server";
const PAIR_OPEN: &str = "open";

const REMOVED: &str = "removed";
const RENAMED: &str = "renamed";
const ADDED: &str = "added";
const TYPE: &str = "type";
const REQUIRED: &str = "required";

const SERVER: &str = "server";
const CLIENT: &str = "client";
const BOTH: &str = "both";

/// The direction an exchange is judged in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Dir {
    Response,
    Request,
    Selection,
}

impl Dir {
    fn as_str(self) -> &'static str {
        match self {
            Dir::Response => "response",
            Dir::Request => "request",
            Dir::Selection => "selection",
        }
    }

    /// The model graph's direction; a direction it adds later is skipped,
    /// never guessed.
    fn of(d: WireDirection) -> Option<Dir> {
        match d {
            WireDirection::Response => Some(Dir::Response),
            WireDirection::Request => Some(Dir::Request),
            _ => None,
        }
    }

    /// The Go wire rule the handler's USES edge of this direction carries.
    fn uses_rule(self) -> Option<&'static str> {
        match self {
            Dir::Response => Some("json_response"),
            Dir::Request => Some("json_bind"),
            Dir::Selection => None,
        }
    }
}

// ---- the answer -------------------------------------------------------------

/// The inferred-model breaks of an already computed rev delta (module docs):
/// [`model_breaks_of`] over `rev.before.merged`, `rev.after` and `rev.delta`,
/// plus the absence when no row is listed. Nothing is built.
pub fn model_breaks(rev: &RevDelta, args: &ModelBreakArgs) -> ModelBreaks {
    let mut out = model_breaks_of(
        &rev.answer.base,
        &rev.before.merged,
        &rev.after,
        &rev.delta,
        args,
    );
    if out.rows.is_empty() {
        let base = &out.base;
        let unjudged = out.exchanges - out.judged;
        let note = if out.exchanges == 0 {
            format!(
                "no client <-> server model exchange (an HTTP call or GraphQL operation typed on both sides) in either graph vs {base}"
            )
        } else {
            let mut n = format!(
                "{} client <-> server model {} judged vs {base}, none changed what a client reads or sends",
                out.judged,
                absence::plural(out.judged, "exchange", "exchanges"),
            );
            if unjudged > 0 {
                n.push_str(&format!(
                    "; {unjudged} unjudged (an open or ambiguous side)"
                ));
            }
            n
        };
        let mut a = absence::empty(
            &rev.after.merged,
            PRIMITIVE,
            &format!("rev {base}"),
            "no_match",
            note,
            &["HTTP_CALLS", "GRAPHQL_CALLS"],
            None,
        );
        a.unparsed_files = rev.after.parse_errors.len();
        out.absence = Some(a);
    }
    out
}

/// The body over a rev pair (module docs): `before` is the rev's graph,
/// `after` the working tree's build, `delta` the id-level delta. `absence` is
/// always `None`, so a caller folding the rows into its own answer prints no
/// extra `[absence]` line. Prints the fired_on marker.
pub(crate) fn model_breaks_of(
    base: &str,
    before: &MergedGraph,
    after: &GenerateResult,
    delta: &GraphDelta,
    args: &ModelBreakArgs,
) -> ModelBreaks {
    let min_tier = if args.min_tier == check::DERIVED {
        check::DERIVED
    } else {
        check::FACT
    };
    let now_m = &after.merged;
    let moved: HashMap<u64, u64> = delta.moved_nodes.iter().map(|&(b, a)| (b.0, a.0)).collect();
    let (then, now) = (Locator::new(before), Locator::new(now_m));
    let (then_nodes, now_nodes) = (NodeIndex::new(before), NodeIndex::new(now_m));
    let then_side = Rev {
        m: before,
        loc: &then,
        nodes: &then_nodes,
    };
    let now_side = Rev {
        m: now_m,
        loc: &now,
        nodes: &now_nodes,
    };
    let old = exchanges_of(&then_side);
    let new = exchanges_of(&now_side);
    let http = new.iter().filter(|e| e.pairing == HTTP_MODEL).count();
    let graphql = new.len() - http;
    let map = |id: u64| moved.get(&id).copied().unwrap_or(id);
    let matched = pair_exchanges(&old, &new, &map);

    let edges = EdgeIndex::new(now_m);
    let mut judged: Vec<JudgedExchange> = Vec::new();
    let (mut unjudged, mut new_pairings) = (0usize, 0usize);
    for (ai, ex) in new.iter().enumerate() {
        let base_ex = matched[ai].map(|bi| &old[bi]);
        if base_ex.is_none() {
            new_pairings += 1;
        }
        if !ex.judgeable || base_ex.is_some_and(|b| !b.judgeable) {
            unjudged += 1;
            continue;
        }
        let tier = exchange_tier(&edges, ex);
        let loc = FieldLocator {
            then: &then_side,
            now: &now_side,
            moved: &moved,
            ex,
            base: base_ex,
        };
        let (changes, open) = match base_ex {
            Some(b) => {
                let j = judge(
                    ex.dir,
                    Some(&b.client),
                    &ex.client,
                    Some(&b.server),
                    &ex.server,
                    ex.fold,
                );
                let located = j.verdicts.iter().map(|v| loc.change(v)).collect();
                (located, j.open)
            }
            None => (static_changes(ex, &loc), false),
        };
        judged.push(JudgedExchange {
            ex: ai,
            new: base_ex.is_none(),
            tier,
            changes,
            open,
        });
    }
    let lost = old.len() - matched.iter().flatten().count();

    let mut rows = rows_of(&new, &judged, &now_side, min_tier);
    rows.sort_by(|a, b| row_key(a).cmp(&row_key(b)));
    let breaking = rows.iter().filter(|r| r.status == "breaking").count();
    let blocking = rows.iter().filter(|r| r.blocking).count();
    let compatible = rows.len() - breaking;
    let marker = format!(
        "[model-breaks] base={base} min_tier={min_tier} exchanges={} http={http} graphql={graphql} rows={} breaking={breaking} blocking={blocking} advisory={} compatible={compatible} new_pairings={new_pairings} lost={lost} unjudged={unjudged}",
        new.len(),
        rows.len(),
        breaking - blocking,
    );
    eprintln!("{marker}");
    ModelBreaks {
        base: base.to_string(),
        min_tier,
        rows,
        exchanges: new.len(),
        judged: new.len() - unjudged,
        breaking,
        blocking,
        advisory: breaking - blocking,
        compatible,
        absence: None,
        marker,
    }
}

/// One working-tree exchange that was judged.
struct JudgedExchange {
    /// Index into the working tree's exchanges.
    ex: usize,
    new: bool,
    tier: &'static str,
    changes: Vec<ModelChange>,
    open: bool,
}

type RowKey<'a> = (u8, &'static str, &'static str, &'a str, &'a str);

fn row_key(r: &ModelBreak) -> RowKey<'_> {
    (
        u8::from(r.status != "breaking"),
        r.pairing,
        r.direction,
        r.client.qname.as_str(),
        r.server.qname.as_str(),
    )
}

/// The rows (module docs ROWS), unsorted.
fn rows_of(
    exchanges: &[Exchange],
    judged: &[JudgedExchange],
    now: &Rev<'_>,
    min_tier: &'static str,
) -> Vec<ModelBreak> {
    let mut groups: BTreeMap<(&'static str, Dir, u64, u64), Vec<&JudgedExchange>> = BTreeMap::new();
    for j in judged {
        let ex = &exchanges[j.ex];
        groups
            .entry((ex.pairing, ex.dir, ex.client_root, ex.server_root))
            .or_default()
            .push(j);
    }
    let mut rows = Vec::new();
    for ((pairing, dir, client_root, server_root), mut members) in groups {
        members.sort_by(|a, b| {
            let (x, y) = (&exchanges[a.ex], &exchanges[b.ex]);
            // The server site breaks a tie (two handlers of one route), so
            // the order is the content's, never a NodeId's.
            (&x.key, &x.client_site, &x.server_site).cmp(&(&y.key, &y.client_site, &y.server_site))
        });
        let mut changes: Vec<ModelChange> = Vec::new();
        let mut seen: BTreeSet<(String, &'static str, Option<String>, Option<String>)> =
            BTreeSet::new();
        for j in &members {
            for c in &j.changes {
                if seen.insert((c.field.clone(), c.rule, c.before.clone(), c.after.clone())) {
                    changes.push(c.clone());
                }
            }
        }
        if changes.is_empty() {
            continue;
        }
        changes.sort_by(|a, b| (&a.field, a.rule).cmp(&(&b.field, b.rule)));
        let all_new = members.iter().all(|j| j.new);
        let status = if changes.iter().any(|c| c.breaking) {
            "breaking"
        } else {
            "compatible"
        };
        let changed = if all_new {
            "pairing"
        } else if changes.iter().all(|c| c.side == SERVER) {
            SERVER
        } else if changes.iter().all(|c| c.side == CLIENT) {
            CLIENT
        } else {
            BOTH
        };
        let note = if all_new {
            Some(NOTE_NEW_PAIRING)
        } else {
            members.iter().any(|j| j.open).then_some(NOTE_OPEN)
        };
        let tier = members
            .iter()
            .map(|j| j.tier)
            .min_by_key(|t| tier_rank(t))
            .unwrap_or(check::DERIVED);
        let first = &exchanges[members[0].ex];
        let client = now.side(client_root, first.client_format, first.pairing);
        let server = now.side(server_root, first.server_format, first.pairing);
        let exchanges_out = members
            .iter()
            .map(|j| {
                let ex = &exchanges[j.ex];
                ModelExchange {
                    key: ex.key.clone(),
                    tier: j.tier,
                    client_site: ex.client_site.clone(),
                    server_site: ex.server_site.clone(),
                }
            })
            .collect();
        rows.push(ModelBreak {
            pairing,
            direction: dir.as_str(),
            client,
            server,
            exchanges: exchanges_out,
            status,
            change: if all_new { ADDED } else { "modified" },
            changed,
            tier,
            blocking: status == "breaking" && check::is_blocking(tier, min_tier),
            note,
            changes,
        });
    }
    rows
}

/// Sort rank of a tier, strongest first; an unknown spelling sorts last.
fn tier_rank(t: &str) -> u8 {
    match t {
        check::FACT => 0,
        check::DERIVED => 1,
        check::HEURISTIC => 2,
        _ => 3,
    }
}

// ---- one side's graph --------------------------------------------------------

/// The first node (in graph order) of every id of one merged graph.
struct NodeIndex<'m>(HashMap<u64, &'m Node>);

impl<'m> NodeIndex<'m> {
    fn new(m: &'m MergedGraph) -> Self {
        let mut at: HashMap<u64, &'m Node> = HashMap::new();
        for g in &m.graphs {
            for n in &g.nodes {
                at.entry(n.id.0).or_insert(n);
            }
        }
        NodeIndex(at)
    }

    fn node(&self, id: u64) -> Option<&'m Node> {
        self.0.get(&id).copied()
    }

    /// The node's in-memory CODE text.
    fn code(&self, id: u64) -> Option<&'m str> {
        self.node(id)?
            .cells
            .iter()
            .filter(|c| c.kind == cell_type::CODE)
            .find_map(|c| match &c.payload {
                CellPayload::Text(t) | CellPayload::Json(t) => Some(t.as_str()),
                _ => None,
            })
    }

    /// The format of the node's SCHEMA_FIELDS.
    fn format(&self, id: u64) -> Option<WireFormat> {
        self.node(id)?
            .cells
            .iter()
            .find_map(ModelSchema::from_cell)
            .map(|s| s.format)
    }
}

/// One side of the rev: its graph, its one Locator, its node index.
struct Rev<'a> {
    m: &'a MergedGraph,
    loc: &'a Locator<'a>,
    nodes: &'a NodeIndex<'a>,
}

impl Rev<'_> {
    /// `id` located, as a site.
    fn site(&self, id: u64, at_base: bool) -> ModelSite {
        let at = self.loc.locate(NodeId(id));
        ModelSite {
            file: at.file,
            line: at.line,
            at_base,
        }
    }

    /// A row's side: `id` located, its schema's format, else the site's
    /// language (`graphql` for a GraphQL side).
    fn side(&self, id: u64, site: Option<WireFormat>, pairing: &str) -> FieldSide {
        let at = self.loc.locate(NodeId(id));
        let format = self
            .nodes
            .format(id)
            .or(site)
            .or((pairing == GRAPHQL_SELECTION).then_some(WireFormat::Graphql))
            .map(|f| f.as_str().to_string())
            .unwrap_or_default();
        FieldSide {
            repo_id: self.nodes.node(id).map_or(0, |n| n.repo.0),
            qname: at.qname,
            format,
            file: at.file,
            line: at.line,
        }
    }
}

// ---- exchanges ------------------------------------------------------------------

/// One static breaking change DT.1m / DT.1n found: what a new pairing lists.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StaticChange {
    /// The flattened path as [`WireFields`] keys it.
    path: String,
    rule: &'static str,
    /// The client's declaration (a selection's `key: field` text).
    client: Option<String>,
    server: Option<String>,
}

/// One exchange of one side (module docs EXCHANGES).
#[derive(Debug, Clone)]
struct Exchange {
    pairing: &'static str,
    dir: Dir,
    key: String,
    route: u64,
    handler: u64,
    endpoint: u64,
    file: String,
    line: u32,
    client_root: u64,
    server_root: u64,
    client_format: Option<WireFormat>,
    server_format: Option<WireFormat>,
    client: WireFields,
    server: WireFields,
    /// A Go binder folds request names.
    fold: bool,
    judgeable: bool,
    statics: Vec<StaticChange>,
    client_site: ModelSite,
    server_site: ModelSite,
    /// GraphQL: the op (whose CODE holds the selection) and its root field.
    op: Option<(u64, String)>,
}

type Ident = (&'static str, Dir, u64, u64, u64, String);

impl Exchange {
    fn ident(&self, map: &dyn Fn(u64) -> u64) -> Ident {
        (
            self.pairing,
            self.dir,
            map(self.route),
            map(self.handler),
            map(self.endpoint),
            self.file.clone(),
        )
    }
}

/// `[].x` (DT.1m's root-array element path) as [`WireFields`] keys it (`x`).
fn wire_path(p: &str) -> String {
    match p {
        "[]" => String::new(),
        _ => p.strip_prefix("[].").unwrap_or(p).to_string(),
    }
}

/// Every exchange of one side, deduplicated by ident (the first by line).
fn exchanges_of(rev: &Rev<'_>) -> Vec<Exchange> {
    let m = rev.m;
    let mut out: Vec<Exchange> = Vec::new();
    let wp = wire_pairs(m);
    if !wp.pairs.is_empty() {
        let cw = client_wires(m);
        let by_site: HashMap<(u64, &str, u32), &ClientWire> = cw
            .wires
            .iter()
            .map(|w| ((w.endpoint.0, w.file.as_str(), w.line), w))
            .collect();
        for p in &wp.pairs {
            let Some(dir) = Dir::of(p.direction) else {
                continue;
            };
            let wire = by_site.get(&(p.endpoint.0, p.file.as_str(), p.line));
            let client = wire.and_then(|w| w.client(p.direction));
            let site = wire
                .and_then(|w| w.routes.iter().find(|r| r.route == p.route))
                .and_then(|r| r.handlers.iter().find(|h| h.handler == p.handler))
                .and_then(|h| {
                    h.sites(p.direction)
                        .iter()
                        .find(|s| Some(s.line) == p.server_line)
                });
            // DT.1p: a union client root is flattened as the member the
            // server value meets (`ResolvedShape::judged_against`).
            let cf = match (client, site) {
                (Some(c), Some(s)) => wire_fields(&c.judged_against(&s.shape)),
                (Some(c), None) => wire_fields(c),
                (None, _) => WireFields::default(),
            };
            let sf = site.map(|s| wire_fields(&s.shape)).unwrap_or_default();
            let judgeable = !p.notes.contains(&PAIR_AMBIGUOUS)
                && client.is_some()
                && site.is_some()
                && !cf.open.contains("")
                && !sf.open.contains("");
            let statics = p
                .changes
                .iter()
                .filter(|c| c.breaking)
                .map(|c| StaticChange {
                    path: wire_path(&c.path),
                    rule: c.rule,
                    client: c.client.clone(),
                    server: c.server.clone(),
                })
                .collect();
            let handler_file = rev.loc.file_of(p.handler);
            out.push(Exchange {
                pairing: HTTP_MODEL,
                dir,
                key: split_owner(&p.route_qname).0.to_string(),
                route: p.route.0,
                handler: p.handler.0,
                endpoint: p.endpoint.0,
                file: p.file.clone(),
                line: p.line,
                client_root: p.client_model.unwrap_or(p.endpoint).0,
                server_root: p.server_model.unwrap_or(p.handler).0,
                client_format: p.client_format,
                server_format: p.server_format,
                client: cf,
                server: sf,
                fold: dir == Dir::Request && p.server_format == Some(WireFormat::Go),
                judgeable,
                statics,
                client_site: ModelSite {
                    file: Some(p.file.clone()),
                    line: (p.line > 0).then_some(i64::from(p.line)),
                    at_base: false,
                },
                server_site: ModelSite {
                    file: handler_file,
                    line: p.server_line.map(i64::from),
                    at_base: false,
                },
                op: None,
            });
        }
    }
    let gp = graphql_pairs(m);
    let sides = graphql_fields_all(m, &gp.pairs);
    for (p, (sel, served)) in gp.pairs.iter().zip(sides) {
        let judgeable = p.refused.is_none()
            && !p.notes.contains(&PAIR_OPEN)
            && !sel.open.contains("")
            && !served.open.contains("");
        let statics = p
            .changes
            .iter()
            .filter(|c| c.breaking)
            .map(|c| StaticChange {
                path: c.path.clone(),
                rule: c.rule,
                client: Some(selected_text(
                    &c.key,
                    c.path.rsplit('.').next().unwrap_or(&c.path),
                )),
                server: None,
            })
            .collect();
        let client_site = rev.site(p.op.0, false);
        out.push(Exchange {
            pairing: GRAPHQL_SELECTION,
            dir: Dir::Selection,
            key: format!("{} {} {}", p.op_type, p.op_name, p.root_field),
            route: p.op.0,
            handler: p.resolver.0,
            endpoint: p.op.0,
            file: client_site.file.clone().unwrap_or_default(),
            line: p.line,
            client_root: p.op.0,
            server_root: p.schema_type.unwrap_or(p.resolver).0,
            client_format: Some(WireFormat::Graphql),
            server_format: Some(WireFormat::Graphql),
            client: sel,
            server: served,
            fold: false,
            judgeable,
            statics,
            client_site,
            server_site: rev.site(p.resolver.0, false),
            op: Some((p.op.0, p.root_field.clone())),
        });
    }
    out.sort_by(|a, b| {
        let id = |e: &Exchange| e.ident(&|x| x);
        (id(a), a.line).cmp(&(id(b), b.line))
    });
    out.dedup_by(|later, first| later.ident(&|x| x) == first.ident(&|x| x));
    out
}

/// A selected field's text: `key` alone, or `key: field` for an alias.
fn selected_text(key: &str, field: &str) -> String {
    if key == field {
        key.to_string()
    } else {
        format!("{key}: {field}")
    }
}

/// For each working-tree exchange, the index of the rev's exchange it pairs
/// with (module docs PAIRING ACROSS THE REV); `map` takes a rev id to the
/// working tree's.
fn pair_exchanges(
    old: &[Exchange],
    new: &[Exchange],
    map: &dyn Fn(u64) -> u64,
) -> Vec<Option<usize>> {
    let mut out: Vec<Option<usize>> = vec![None; new.len()];
    let mut taken = vec![false; old.len()];
    let by_ident: HashMap<(Ident, u64), usize> = new
        .iter()
        .enumerate()
        .map(|(i, e)| ((e.ident(&|x| x), e.client_root), i))
        .collect();
    for (bi, b) in old.iter().enumerate() {
        if let Some(&ai) = by_ident.get(&(b.ident(map), map(b.client_root)))
            && out[ai].is_none()
        {
            out[ai] = Some(bi);
            taken[bi] = true;
        }
    }
    type RouteKey<'a> = (&'static str, Dir, &'a str);
    let mut left: BTreeMap<RouteKey<'_>, (Vec<usize>, Vec<usize>)> = BTreeMap::new();
    for (bi, b) in old.iter().enumerate().filter(|(i, _)| !taken[*i]) {
        left.entry((b.pairing, b.dir, b.key.as_str()))
            .or_default()
            .0
            .push(bi);
    }
    for (ai, a) in new.iter().enumerate().filter(|(i, _)| out[*i].is_none()) {
        left.entry((a.pairing, a.dir, a.key.as_str()))
            .or_default()
            .1
            .push(ai);
    }
    for (bs, as_) in left.values() {
        if let ([b], [a]) = (bs.as_slice(), as_.as_slice()) {
            out[*a] = Some(*b);
        }
    }
    out
}

// ---- tier -----------------------------------------------------------------------

/// The working tree's edges of the joining categories, by (from, to, category).
struct EdgeIndex<'m>(HashMap<(u64, u64, EdgeCategoryId), Vec<&'m Edge>>);

const JOINING: [EdgeCategoryId; 4] = [
    edge_category::HTTP_CALLS,
    edge_category::HANDLED_BY,
    edge_category::USES,
    edge_category::GRAPHQL_CALLS,
];

impl<'m> EdgeIndex<'m> {
    fn new(m: &'m MergedGraph) -> Self {
        let mut at: HashMap<(u64, u64, EdgeCategoryId), Vec<&'m Edge>> = HashMap::new();
        for e in m.all_edges().filter(|e| JOINING.contains(&e.category)) {
            at.entry((e.from.0, e.to.0, e.category))
                .or_default()
                .push(e);
        }
        EdgeIndex(at)
    }

    /// The strongest tier of the `category` edges `from` -> `to` (only those
    /// whose EVIDENCE rule is `rule`, when given); derived when none joins.
    fn tier(
        &self,
        from: u64,
        to: u64,
        category: EdgeCategoryId,
        rule: Option<&str>,
    ) -> &'static str {
        self.0
            .get(&(from, to, category))
            .into_iter()
            .flatten()
            .filter_map(|e| {
                let ev = Evidence::of(e);
                let keep =
                    rule.is_none_or(|r| ev.as_ref().and_then(|v| v.rule.as_deref()) == Some(r));
                keep.then(|| tier_of(ev.as_ref(), e).0)
            })
            .min_by_key(|t| tier_rank(t))
            .unwrap_or(check::DERIVED)
    }
}

/// An exchange's tier (module docs TIER AND GATE): per joining category the
/// strongest edge, then the weakest category.
fn exchange_tier(edges: &EdgeIndex<'_>, ex: &Exchange) -> &'static str {
    let mut tiers: Vec<&'static str> = Vec::new();
    match ex.dir {
        Dir::Selection => {
            tiers.push(edges.tier(ex.endpoint, ex.handler, edge_category::GRAPHQL_CALLS, None));
        }
        Dir::Response | Dir::Request => {
            tiers.push(edges.tier(ex.endpoint, ex.route, edge_category::HTTP_CALLS, None));
            tiers.push(edges.tier(ex.route, ex.handler, edge_category::HANDLED_BY, None));
            if ex.server_root != ex.handler {
                tiers.push(edges.tier(
                    ex.handler,
                    ex.server_root,
                    edge_category::USES,
                    ex.dir.uses_rule(),
                ));
            }
        }
    }
    tiers
        .into_iter()
        .max_by_key(|t| tier_rank(t))
        .unwrap_or(check::DERIVED)
}

// ---- the rules ------------------------------------------------------------------

/// Which side's field a verdict is located at, and in which graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Now,
    Base,
}

/// One judged change, before its field lines are located.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Verdict {
    /// The path (a rename's old one), as [`WireFields`] keys it.
    path: String,
    change: &'static str,
    side: &'static str,
    rule: &'static str,
    breaking: bool,
    before: Option<String>,
    after: Option<String>,
    /// The client side's key and graph, when the client lists the field.
    client: Option<(String, At)>,
    /// The server side's key and graph, when the server lists the field.
    server: Option<(String, At)>,
}

/// What [`judge`] found: its verdicts, sorted by (path, rule), and whether a
/// claim needing an absence was withheld below an open path.
#[derive(Debug, Default)]
struct Judged {
    verdicts: Vec<Verdict>,
    open: bool,
}

/// `p` lies strictly below `root` (`root` `""` holds every other path).
fn under(p: &str, root: &str) -> bool {
    p != root
        && (root.is_empty()
            || p.strip_prefix(root)
                .is_some_and(|r| r.starts_with('.') || r.starts_with("[]")))
}

/// `p` lies below an open path of `w`: no name there may be claimed absent.
fn covered(w: &WireFields, p: &str) -> bool {
    w.open.iter().any(|o| under(p, o))
}

/// The field at `p`: exact, else (a Go binder) the least ASCII
/// case-insensitive match.
fn get<'w>(w: &'w WireFields, p: &str, fold: bool) -> Option<(&'w str, &'w WireField)> {
    if let Some((k, f)) = w.fields.get_key_value(p) {
        return Some((k.as_str(), f));
    }
    if !fold {
        return None;
    }
    w.fields
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(p))
        .map(|(k, f)| (k.as_str(), f))
}

/// A kind that is judged: known and not `any`.
fn known(k: Option<&'static str>) -> Option<&'static str> {
    k.filter(|k| *k != "any")
}

/// What a side's field is compared by: presence, kind and requiredness (a
/// request's server side: its binder's).
fn attrs(f: Option<&WireField>, dir: Dir, server: bool) -> Option<(Option<&'static str>, bool)> {
    f.map(|f| {
        let req = if dir == Dir::Request && server {
            f.bind_required
        } else {
            f.required
        };
        (f.kind, req)
    })
}

fn moved(a: Option<&WireField>, b: Option<&WireField>, dir: Dir, server: bool) -> bool {
    attrs(a, dir, server) != attrs(b, dir, server)
}

/// The parent path of `p` (`lines[]` for `lines[].sku`, `""` at the top).
fn parent(p: &str) -> &str {
    p.rsplit_once('.').map_or("", |(a, _)| a)
}

/// `required` / `optional`, or GraphQL's `non-null` / `nullable`.
fn req_word(dir: Dir, required: bool) -> String {
    match (dir, required) {
        (Dir::Selection, true) => "non-null",
        (Dir::Selection, false) => "nullable",
        (_, true) => "required",
        (_, false) => "optional",
    }
    .to_string()
}

/// The verdicts a pass pushes, with the client side looked up once.
struct Pass<'a> {
    dir: Dir,
    fold: bool,
    c0: &'a WireFields,
    c1: &'a WireFields,
    out: Judged,
}

impl Pass<'_> {
    /// Where the client lists `p`: now, else at the rev.
    fn client_at(&self, p: &str) -> Option<(String, At)> {
        get(self.c1, p, self.fold)
            .map(|(k, _)| (k.to_string(), At::Now))
            .or_else(|| get(self.c0, p, self.fold).map(|(k, _)| (k.to_string(), At::Base)))
    }

    fn client_moved(&self, p: &str) -> bool {
        moved(
            get(self.c0, p, self.fold).map(|x| x.1),
            get(self.c1, p, self.fold).map(|x| x.1),
            self.dir,
            false,
        )
    }

    fn push(&mut self, v: Verdict) {
        self.out.verdicts.push(v);
    }
}

impl Verdict {
    /// A verdict at `path`, located nowhere yet ([`Verdict::at`]).
    fn new(
        path: &str,
        change: &'static str,
        side: &'static str,
        rule: &'static str,
        breaking: bool,
        (before, after): (Option<String>, Option<String>),
    ) -> Verdict {
        Verdict {
            path: path.to_string(),
            change,
            side,
            rule,
            breaking,
            before,
            after,
            client: None,
            server: None,
        }
    }

    /// The verdict located at the client's and the server's keys.
    fn at(mut self, client: Option<(String, At)>, server: Option<(String, At)>) -> Verdict {
        self.client = client;
        self.server = server;
        self
    }
}

/// The rules table (module docs THE RULES): the changes between a client
/// and a server side at the rev (`c0`, `s0`) and in the working tree (`c1`,
/// `s1`). `None` is a side that did not move (its working-tree copy).
/// `fold`: a Go binder's case-insensitive names (a request only).
fn judge(
    dir: Dir,
    c0: Option<&WireFields>,
    c1: &WireFields,
    s0: Option<&WireFields>,
    s1: &WireFields,
    fold: bool,
) -> Judged {
    let c0 = c0.unwrap_or(c1);
    let s0 = s0.unwrap_or(s1);
    let mut pass = Pass {
        dir,
        fold,
        c0,
        c1,
        out: Judged::default(),
    };
    let mut roots: Vec<&str> = Vec::new();
    let mut consumed: BTreeSet<&str> = BTreeSet::new();
    let mut handled: BTreeSet<&str> = BTreeSet::new();
    let paths: BTreeSet<&str> = s0
        .fields
        .keys()
        .chain(s1.fields.keys())
        .map(String::as_str)
        .collect();

    // Server-side changes, judged against what the client reads or sends now.
    for &p in &paths {
        let (a, b) = (s0.fields.get(p), s1.fields.get(p));
        if !moved(a, b, dir, true) {
            continue;
        }
        handled.insert(p);
        if consumed.contains(p) || roots.iter().any(|r| under(p, r)) {
            continue;
        }
        let now = get(c1, p, fold).map(|x| x.1);
        let side = if pass.client_moved(p) { BOTH } else { SERVER };
        match (a, b) {
            (Some(a), None) => {
                roots.push(p);
                if covered(s1, p) {
                    pass.out.open = true;
                    continue;
                }
                let rename = (dir != Dir::Selection)
                    .then(|| {
                        s1.fields.iter().find(|(q, f)| {
                            parent(q) == parent(p)
                                && f.member == a.member
                                && !s0.fields.contains_key(q.as_str())
                                && !covered(s0, q)
                                && !consumed.contains(q.as_str())
                        })
                    })
                    .flatten();
                if let Some((q, qf)) = rename {
                    let q = q.as_str();
                    consumed.insert(q);
                    roots.push(q);
                    let lacks_q = get(c1, q, fold).is_none();
                    if now.is_some() && lacks_q && covered(c1, q) {
                        pass.out.open = true;
                    }
                    let breaking = now.is_some() && lacks_q && !covered(c1, q);
                    let side = if side == BOTH || pass.client_moved(q) {
                        BOTH
                    } else {
                        SERVER
                    };
                    let rule = match dir {
                        Dir::Request => "request_json_name_changed",
                        _ => "json_name_changed",
                    };
                    let client = pass.client_at(p);
                    pass.push(
                        Verdict::new(
                            p,
                            RENAMED,
                            side,
                            rule,
                            breaking,
                            (Some(a.name.clone()), Some(qf.name.clone())),
                        )
                        .at(client, Some((q.to_string(), At::Now))),
                    );
                    continue;
                }
                let (rule, breaking) = match dir {
                    Dir::Response => ("response_field_removed", now.is_some()),
                    Dir::Request => ("request_field_removed", false),
                    Dir::Selection if now.is_some() => ("schema_field_removed", true),
                    // A schema field nobody selects is never a change.
                    Dir::Selection => continue,
                };
                let client = pass.client_at(p);
                pass.push(
                    Verdict::new(p, REMOVED, side, rule, breaking, (Some(a.ty.clone()), None))
                        .at(client, Some((p.to_string(), At::Base))),
                );
            }
            (None, Some(b)) => {
                roots.push(p);
                if covered(s0, p) {
                    pass.out.open = true;
                    continue;
                }
                let (rule, breaking) = match dir {
                    Dir::Response => ("response_field_added", false),
                    Dir::Request if b.bind_required => {
                        if now.is_some() {
                            continue;
                        }
                        if covered(c1, p) {
                            pass.out.open = true;
                            continue;
                        }
                        ("new_required_request_field", true)
                    }
                    Dir::Request => ("request_field_added", false),
                    Dir::Selection => continue,
                };
                let client = pass.client_at(p);
                pass.push(
                    Verdict::new(p, ADDED, side, rule, breaking, (None, Some(b.ty.clone())))
                        .at(client, Some((p.to_string(), At::Now))),
                );
            }
            (Some(a), Some(b)) => {
                let retyped =
                    known(a.kind).is_some() && known(b.kind).is_some() && a.kind != b.kind;
                if retyped {
                    roots.push(p);
                }
                let client = pass.client_at(p);
                let server = Some((p.to_string(), At::Now));
                let types = (Some(a.ty.clone()), Some(b.ty.clone()));
                match (dir, now) {
                    (Dir::Response, Some(c)) => {
                        if retyped {
                            let breaking = known(c.kind).is_some_and(|k| Some(k) != b.kind);
                            let (cl, sv) = (client.clone(), server.clone());
                            pass.push(
                                Verdict::new(
                                    p,
                                    TYPE,
                                    side,
                                    "response_type_changed",
                                    breaking,
                                    types,
                                )
                                .at(cl, sv),
                            );
                        }
                        if a.required && !b.required {
                            let words = (Some(req_word(dir, true)), Some(req_word(dir, false)));
                            let breaking = c.required;
                            pass.push(
                                Verdict::new(
                                    p,
                                    REQUIRED,
                                    side,
                                    "response_field_now_optional",
                                    breaking,
                                    words,
                                )
                                .at(client, server),
                            );
                        }
                    }
                    (Dir::Request, now) => {
                        if !a.bind_required && b.bind_required && now.is_none() {
                            if covered(c1, p) {
                                pass.out.open = true;
                            } else {
                                let words = (Some(req_word(dir, false)), Some(req_word(dir, true)));
                                let (cl, sv) = (client.clone(), server.clone());
                                pass.push(
                                    Verdict::new(
                                        p,
                                        REQUIRED,
                                        side,
                                        "new_required_request_field",
                                        true,
                                        words,
                                    )
                                    .at(cl, sv),
                                );
                            }
                        }
                        if let Some(c) = now
                            && a.kind != b.kind
                            && known(b.kind).is_some()
                            && known(c.kind).is_some_and(|k| Some(k) != b.kind)
                        {
                            pass.push(
                                Verdict::new(p, TYPE, side, "request_type_changed", true, types)
                                    .at(client, server),
                            );
                        }
                    }
                    (Dir::Selection, Some(_)) => {
                        if retyped {
                            let (cl, sv) = (client.clone(), server.clone());
                            pass.push(
                                Verdict::new(
                                    p,
                                    TYPE,
                                    side,
                                    "schema_field_type_changed",
                                    true,
                                    types,
                                )
                                .at(cl, sv),
                            );
                        }
                        if a.required && !b.required {
                            let words = (Some(req_word(dir, true)), Some(req_word(dir, false)));
                            pass.push(
                                Verdict::new(
                                    p,
                                    REQUIRED,
                                    side,
                                    "schema_field_now_nullable",
                                    true,
                                    words,
                                )
                                .at(client, server),
                            );
                        }
                    }
                    (Dir::Response | Dir::Selection, None) => {}
                }
            }
            (None, None) => {}
        }
    }

    // Client-side changes, where the server did not move.
    let mut croots: Vec<&str> = Vec::new();
    let cpaths: BTreeSet<&str> = c0
        .fields
        .keys()
        .chain(c1.fields.keys())
        .map(String::as_str)
        .collect();
    for &p in &cpaths {
        let (a, b) = (c0.fields.get(p), c1.fields.get(p));
        if !moved(a, b, dir, false) {
            continue;
        }
        let at_server = get(s1, p, fold).or_else(|| get(s0, p, fold)).map(|x| x.0);
        if at_server.is_some_and(|k| handled.contains(k)) || croots.iter().any(|r| under(p, r)) {
            continue;
        }
        let s = get(s1, p, fold);
        let server = s.map(|(k, _)| (k.to_string(), At::Now));
        match (a, b) {
            (None, Some(b)) => {
                croots.push(p);
                if covered(c0, p) {
                    pass.out.open = true;
                    continue;
                }
                if s.is_some() {
                    continue;
                }
                if covered(s1, p) {
                    pass.out.open = true;
                    continue;
                }
                let (rule, breaking, after) = match dir {
                    Dir::Response if b.required => ("json_field_missing", true, b.ty.clone()),
                    Dir::Response => ("json_field_optional_absent", false, b.ty.clone()),
                    Dir::Request => ("json_field_dropped", false, b.ty.clone()),
                    Dir::Selection => (
                        "graphql_field_undefined",
                        true,
                        selected_text(&b.name, &b.member),
                    ),
                };
                let client = Some((p.to_string(), At::Now));
                pass.push(
                    Verdict::new(p, ADDED, CLIENT, rule, breaking, (None, Some(after)))
                        .at(client, None),
                );
            }
            (Some(a), None) => {
                croots.push(p);
                if covered(c1, p) {
                    pass.out.open = true;
                    continue;
                }
                let (Dir::Request, Some((_, sf))) = (dir, s) else {
                    continue;
                };
                let (rule, breaking) = if sf.bind_required {
                    ("json_required_missing", true)
                } else {
                    ("json_field_unset", false)
                };
                let client = Some((p.to_string(), At::Base));
                pass.push(
                    Verdict::new(
                        p,
                        REMOVED,
                        CLIENT,
                        rule,
                        breaking,
                        (Some(a.ty.clone()), None),
                    )
                    .at(client, server),
                );
            }
            (Some(a), Some(b)) => {
                let Some((_, sf)) = s else {
                    continue;
                };
                let client = Some((p.to_string(), At::Now));
                if dir != Dir::Selection
                    && a.kind != b.kind
                    && known(sf.kind).is_some()
                    && known(b.kind).is_some_and(|k| Some(k) != sf.kind)
                {
                    croots.push(p);
                    let types = (Some(a.ty.clone()), Some(b.ty.clone()));
                    let (cl, sv) = (client.clone(), server.clone());
                    pass.push(
                        Verdict::new(p, TYPE, CLIENT, "json_type_mismatch", true, types).at(cl, sv),
                    );
                }
                if dir == Dir::Response && !a.required && b.required && !sf.required {
                    let words = (Some(req_word(dir, false)), Some(req_word(dir, true)));
                    pass.push(
                        Verdict::new(
                            p,
                            REQUIRED,
                            CLIENT,
                            "json_field_may_be_absent",
                            false,
                            words,
                        )
                        .at(client, server),
                    );
                }
            }
            (None, None) => {}
        }
    }
    let mut out = pass.out;
    out.verdicts
        .sort_by(|a, b| (&a.path, a.rule).cmp(&(&b.path, b.rule)));
    out
}

// ---- field lines ----------------------------------------------------------------

/// The root path as a row shows it.
fn display(p: &str) -> String {
    if p.is_empty() {
        "(root)".to_string()
    } else {
        p.to_string()
    }
}

/// The last segment of a path: the JSON name it ends in.
fn segment(p: &str) -> &str {
    p.rsplit('.').next().unwrap_or(p)
}

/// The members of a GraphQL selection path below its root field.
fn segments(p: &str) -> Vec<&str> {
    if p.is_empty() {
        Vec::new()
    } else {
        p.split('.').collect()
    }
}

/// Locates one exchange's fields (module docs ROWS; [`member_site`],
/// [`selection_site`]).
struct FieldLocator<'a> {
    then: &'a Rev<'a>,
    now: &'a Rev<'a>,
    moved: &'a HashMap<u64, u64>,
    ex: &'a Exchange,
    base: Option<&'a Exchange>,
}

impl FieldLocator<'_> {
    fn change(&self, v: &Verdict) -> ModelChange {
        ModelChange {
            field: display(&v.path),
            change: v.change,
            side: v.side,
            rule: v.rule,
            breaking: v.breaking,
            before: v.before.clone(),
            after: v.after.clone(),
            client: v.client.as_ref().and_then(|(k, at)| self.client(k, *at)),
            server: v.server.as_ref().and_then(|(k, at)| self.server(k, *at)),
        }
    }

    /// The client's line for key `k`: the selection line in the op's CODE
    /// (GraphQL), else the declaring node's member line, else the exchange's
    /// call site (an inline value, the root).
    fn client(&self, k: &str, at: At) -> Option<ModelSite> {
        let (rev, ex, at_base) = match (at, self.base) {
            (At::Base, Some(b)) => (self.then, b, true),
            _ => (self.now, self.ex, false),
        };
        if let Some((op, root)) = &ex.op {
            let mut path = vec![root.as_str()];
            path.extend(segments(k));
            return selection_site(rev, *op, &path).map(|mut s| {
                s.at_base = at_base;
                s
            });
        }
        let Some(f) = ex.client.fields.get(k) else {
            return Some(self.ex.client_site.clone());
        };
        self.field_line(rev, f, k, at_base, ex.client_root, &self.ex.client_site)
    }

    /// The server's line for key `k`: GraphQL's schema type line, else the
    /// declaring node's member line, else the handler site.
    fn server(&self, k: &str, at: At) -> Option<ModelSite> {
        let (rev, ex, at_base) = match (at, self.base) {
            (At::Base, Some(b)) => (self.then, b, true),
            _ => (self.now, self.ex, false),
        };
        let Some(f) = ex.server.fields.get(k) else {
            return Some(self.ex.server_site.clone());
        };
        if ex.op.is_some() {
            let Some(d) = f.declared_by else {
                return Some(self.ex.server_site.clone());
            };
            let mapped = self.map(d.0);
            if self.now.nodes.node(mapped).is_some() {
                return Some(self.now.site(mapped, false));
            }
            return Some(rev.site(d.0, at_base));
        }
        self.field_line(rev, f, k, at_base, ex.server_root, &self.ex.server_site)
    }

    fn map(&self, id: u64) -> u64 {
        self.moved.get(&id).copied().unwrap_or(id)
    }

    /// A field's line. Read now: its declaring node's member line, an inline
    /// key at `site`. Only at the rev (a removed field): the working tree's
    /// declaring node when its CODE still declares the member (a `json:"-"`
    /// tag), else the rev's member line (at_base), else the working tree
    /// root's own line.
    fn field_line(
        &self,
        rev: &Rev<'_>,
        f: &WireField,
        k: &str,
        at_base: bool,
        root: u64,
        site: &ModelSite,
    ) -> Option<ModelSite> {
        let Some(d) = f.declared_by else {
            // The root has no declaring node: its model's own line. An inline
            // value's key: the exchange's site.
            if let (true, Some(m)) = (k.is_empty(), f.model) {
                let mapped = self.map(m.0);
                if self.now.nodes.node(mapped).is_some() {
                    return Some(self.now.site(mapped, false));
                }
                return Some(rev.site(m.0, at_base));
            }
            return Some(site.clone());
        };
        let name = segment(k);
        if !at_base {
            return member_site(rev, d, name, &f.member).map(|(s, _)| s);
        }
        let mapped = NodeId(self.map(d.0));
        if let Some((s, true)) = member_site(self.now, mapped, name, &f.member) {
            return Some(s);
        }
        if let Some((mut s, _)) = member_site(rev, d, name, &f.member) {
            s.at_base = true;
            return Some(s);
        }
        let root = self.map(root);
        Some(if self.now.nodes.node(root).is_some() {
            self.now.site(root, false)
        } else {
            site.clone()
        })
    }
}

/// The static breaking changes of a new pairing, located in the working tree.
fn static_changes(ex: &Exchange, loc: &FieldLocator<'_>) -> Vec<ModelChange> {
    ex.statics
        .iter()
        .map(|s| {
            let client = get(&ex.client, &s.path, ex.fold).map(|(k, _)| (k.to_string(), At::Now));
            let server = ex
                .server
                .fields
                .contains_key(&s.path)
                .then(|| (s.path.clone(), At::Now));
            loc.change(&Verdict {
                path: s.path.clone(),
                change: ADDED,
                side: CLIENT,
                rule: s.rule,
                breaking: true,
                before: None,
                after: s.client.clone().or_else(|| s.server.clone()),
                client,
                server,
            })
        })
        .collect()
}

/// The located line of the member `member` (JSON name `name`) of `node`:
/// the node's line plus the index of the first line of its CODE that declares
/// or reads it (line 0 is the node's own line), and whether a line matched;
/// the node's own line when none does. `None` when the node is not located.
fn member_site(rev: &Rev<'_>, node: NodeId, name: &str, member: &str) -> Option<(ModelSite, bool)> {
    let at = rev.loc.locate(node);
    let line = at.line?;
    let format = rev.nodes.format(node.0);
    let hit = rev
        .nodes
        .code(node.0)
        .and_then(|code| member_line(code, format, name, member));
    Some((
        ModelSite {
            file: at.file,
            line: Some(line + hit.map_or(0, |i| i as i64)),
            at_base: false,
        },
        hit.is_some(),
    ))
}

/// The CODE line index declaring the field, per format: Go the member
/// (`^\s*<Member>(\s|,|$)`), TS the JSON name (`^\s*(readonly\s+)?['"]?<name>['"]?\??\s*:`),
/// Dart the fromJson subscript (`['<name>']`) else a field declaration
/// (`^\s*(late\s+)?(final\s+)?\S.*\s<member>\s*[;=]`). A one-line CODE is
/// searched from line 0, else from line 1 (line 0 is the declaration).
fn member_line(code: &str, format: Option<WireFormat>, name: &str, member: &str) -> Option<usize> {
    if name.is_empty() && member.is_empty() {
        return None;
    }
    let lines: Vec<&str> = code.lines().collect();
    let from = usize::from(lines.len() > 1);
    let find = |pred: &dyn Fn(&str) -> bool| {
        lines
            .iter()
            .enumerate()
            .skip(from)
            .find(|(_, l)| pred(l))
            .map(|(i, _)| i)
    };
    match format {
        Some(WireFormat::Go) => find(&|l| go_member(l, member)),
        Some(WireFormat::Ts) => find(&|l| ts_member(l, name)),
        Some(WireFormat::Dart) => {
            find(&|l| l.contains(&format!("['{name}']")) || l.contains(&format!("[\"{name}\"]")))
                .or_else(|| find(&|l| dart_field(l, member)))
        }
        _ => None,
    }
}

/// A Go struct line declaring `member`.
fn go_member(line: &str, member: &str) -> bool {
    line.trim_start()
        .strip_prefix(member)
        .is_some_and(|r| r.is_empty() || r.starts_with(char::is_whitespace) || r.starts_with(','))
}

/// A TS property line declaring `name`.
fn ts_member(line: &str, name: &str) -> bool {
    let mut l = line.trim_start();
    if let Some(r) = l.strip_prefix("readonly")
        && r.starts_with(char::is_whitespace)
    {
        l = r.trim_start();
    }
    let l = l.strip_prefix(['\'', '"']).unwrap_or(l);
    let Some(r) = l.strip_prefix(name) else {
        return false;
    };
    let r = r.strip_prefix(['\'', '"']).unwrap_or(r);
    let r = r.strip_prefix('?').unwrap_or(r);
    r.trim_start().starts_with(':')
}

/// A Dart field declaration line of `member`.
fn dart_field(line: &str, member: &str) -> bool {
    let mut l = line.trim_start();
    for kw in ["late", "final"] {
        if let Some(r) = l.strip_prefix(kw)
            && r.starts_with(char::is_whitespace)
        {
            l = r.trim_start();
        }
    }
    let mut from = 0;
    while let Some(i) = l[from..].find(member) {
        let at = from + i;
        let before = &l[..at];
        let after = l[at + member.len()..].trim_start();
        let typed = before.ends_with(char::is_whitespace) && !before.trim().is_empty();
        if typed && (after.starts_with(';') || after.starts_with('=')) {
            return true;
        }
        from = at + member.len();
    }
    false
}

/// The line of a selected field in the op's CODE (its GraphQL document):
/// each member of `path` (the root field first) is the first line after its
/// parent's, at brace depth one below the parent's selection set, matching
/// `^\s*(<key>\s*:\s*)?<member>\b`; the op's own line when a member is not
/// found. `None` when the op is not located.
fn selection_site(rev: &Rev<'_>, op: u64, path: &[&str]) -> Option<ModelSite> {
    let base = rev.site(op, false);
    let start = base.line?;
    let Some(code) = rev.nodes.code(op) else {
        return Some(base);
    };
    let lines: Vec<&str> = code.lines().collect();
    let mut depth_at = Vec::with_capacity(lines.len());
    let mut depth = 0i64;
    for l in &lines {
        depth_at.push(depth);
        for c in l.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
    }
    let mut from = 0usize;
    let mut hit = 0usize;
    for (level, member) in path.iter().enumerate() {
        let want = level as i64 + 1;
        let mut found = None;
        for i in from..lines.len() {
            // Below the root field the parent's selection set has closed.
            if level > 0 && depth_at[i] < want {
                break;
            }
            if depth_at[i] == want && selects(lines[i], member) {
                found = Some(i);
                break;
            }
        }
        let Some(i) = found else {
            return Some(base);
        };
        hit = i;
        from = i + 1;
    }
    Some(ModelSite {
        line: Some(start + hit as i64),
        ..base
    })
}

/// A selection line naming `member`, with or without an alias.
fn selects(line: &str, member: &str) -> bool {
    let l = line.trim_start();
    let l = match l.split_once(':') {
        Some((alias, rest))
            if !alias.trim().is_empty()
                && alias
                    .trim()
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_') =>
        {
            rest.trim_start()
        }
        _ => l,
    };
    l.strip_prefix(member)
        .is_some_and(|r| !r.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
    use glia_core::{Cell, Confidence, NodeKindId, RepoId};
    use glia_graph::{RepoGraph, SymbolTable};

    const R: Dir = Dir::Response;
    const Q: Dir = Dir::Request;
    const SEL: Dir = Dir::Selection;

    /// A side under construction: the root is an object.
    struct S(WireFields);

    impl S {
        fn new() -> S {
            let mut w = WireFields::default();
            let mut root = WireField::default();
            root.required = true;
            root.kind = Some("object");
            w.fields.insert(String::new(), root);
            S(w)
        }

        /// `path` of `kind` (None: unknown), member = its last segment.
        fn k(self, path: &str, kind: Option<&'static str>, required: bool) -> S {
            let member = segment(path).to_string();
            self.m(path, &member, kind, required)
        }

        fn f(self, path: &str, kind: &'static str, required: bool) -> S {
            self.k(path, Some(kind), required)
        }

        fn m(mut self, path: &str, member: &str, kind: Option<&'static str>, required: bool) -> S {
            let mut f = WireField::default();
            f.name = segment(path).to_string();
            f.member = member.to_string();
            f.kind = kind;
            f.required = required;
            f.ty = kind.unwrap_or("Thing").to_string();
            self.0.fields.insert(path.to_string(), f);
            self
        }

        fn bind(mut self, path: &str) -> S {
            if let Some(f) = self.0.fields.get_mut(path) {
                f.bind_required = true;
            }
            self
        }

        fn open(mut self, path: &str) -> S {
            self.0.open.insert(path.to_string());
            self
        }
    }

    type Row = (String, &'static str, &'static str, &'static str, bool);

    fn rules(j: &Judged) -> Vec<Row> {
        j.verdicts
            .iter()
            .map(|v| (v.path.clone(), v.change, v.side, v.rule, v.breaking))
            .collect()
    }

    fn row(
        p: &str,
        change: &'static str,
        side: &'static str,
        rule: &'static str,
        breaking: bool,
    ) -> Row {
        (p.to_string(), change, side, rule, breaking)
    }

    /// The server moved from `s0` to `s1`; the client is `c` on both sides.
    fn server_moved(dir: Dir, c: &S, s0: &S, s1: &S, fold: bool) -> Judged {
        judge(dir, None, &c.0, Some(&s0.0), &s1.0, fold)
    }

    /// The client moved from `c0` to `c1` against `s`.
    fn client_moved(dir: Dir, c0: &S, c1: &S, s: &S) -> Judged {
        judge(dir, Some(&c0.0), &c1.0, None, &s.0, false)
    }

    // ---- response: server-side changes -----------------------------------

    #[test]
    fn a_response_rename_breaks_a_client_reading_the_old_name() {
        let c = S::new()
            .f("id", "string", true)
            .f("customerName", "string", true);
        let s0 = S::new().f("id", "string", true).m(
            "customerName",
            "CustomerName",
            Some("string"),
            true,
        );
        let s1 = S::new().f("id", "string", true).m(
            "customer_name",
            "CustomerName",
            Some("string"),
            true,
        );
        let j = server_moved(R, &c, &s0, &s1, false);
        assert_eq!(
            rules(&j),
            [row(
                "customerName",
                RENAMED,
                SERVER,
                "json_name_changed",
                true
            )]
        );
        let v = &j.verdicts[0];
        assert_eq!(
            (v.before.as_deref(), v.after.as_deref()),
            (Some("customerName"), Some("customer_name"))
        );
        assert_eq!(v.server, Some(("customer_name".to_string(), At::Now)));
        assert_eq!(v.client, Some(("customerName".to_string(), At::Now)));
        // A client reading the new name already is not hit.
        let both = S::new()
            .f("customerName", "string", false)
            .f("customer_name", "string", true);
        assert_eq!(
            rules(&server_moved(R, &both, &s0, &s1, false)),
            [row(
                "customerName",
                RENAMED,
                SERVER,
                "json_name_changed",
                false
            )]
        );
    }

    #[test]
    fn a_coordinated_rename_is_compatible_on_both_sides() {
        let c0 = S::new().f("customerName", "string", true);
        let c1 = S::new().f("customer_name", "string", true);
        let s0 = S::new().m("customerName", "CustomerName", Some("string"), true);
        let s1 = S::new().m("customer_name", "CustomerName", Some("string"), true);
        let j = judge(R, Some(&c0.0), &c1.0, Some(&s0.0), &s1.0, false);
        assert_eq!(
            rules(&j),
            [row(
                "customerName",
                RENAMED,
                BOTH,
                "json_name_changed",
                false
            )]
        );
    }

    #[test]
    fn a_removed_response_field_breaks_only_its_readers() {
        let c = S::new().f("total", "number", true);
        let s0 = S::new()
            .f("total", "number", true)
            .f("note", "string", true);
        let s1 = S::new();
        let j = server_moved(R, &c, &s0, &s1, false);
        assert_eq!(
            rules(&j),
            [
                row("note", REMOVED, SERVER, "response_field_removed", false),
                row("total", REMOVED, SERVER, "response_field_removed", true),
            ]
        );
        assert_eq!(j.verdicts[1].before.as_deref(), Some("number"));
        assert_eq!(j.verdicts[1].server, Some(("total".to_string(), At::Base)));
    }

    #[test]
    fn an_added_response_field_is_compatible() {
        let c = S::new().f("id", "string", true);
        let s0 = S::new().f("id", "string", true);
        let s1 = S::new().f("id", "string", true).f("notes", "string", false);
        let j = server_moved(R, &c, &s0, &s1, false);
        assert_eq!(
            rules(&j),
            [row("notes", ADDED, SERVER, "response_field_added", false)]
        );
        assert_eq!(j.verdicts[0].client, None);
        assert_eq!(j.verdicts[0].after.as_deref(), Some("string"));
    }

    #[test]
    fn a_retyped_response_field_breaks_a_reader_of_the_old_kind() {
        let s0 = S::new()
            .f("total", "number", true)
            .f("unread", "number", true);
        let s1 = S::new()
            .f("total", "string", true)
            .f("unread", "string", true);
        let old = S::new().f("total", "number", true);
        assert_eq!(
            rules(&server_moved(R, &old, &s0, &s1, false)),
            [row("total", TYPE, SERVER, "response_type_changed", true)],
            "unread is read by no one: not listed"
        );
        let new = S::new().f("total", "string", true);
        assert_eq!(
            rules(&server_moved(R, &new, &s0, &s1, false)),
            [row("total", TYPE, SERVER, "response_type_changed", false)]
        );
        let any = S::new().f("total", "any", true);
        assert_eq!(
            rules(&server_moved(R, &any, &s0, &s1, false)),
            [row("total", TYPE, SERVER, "response_type_changed", false)],
            "an `any` read is no known kind"
        );
    }

    #[test]
    fn a_response_field_made_optional_breaks_a_client_that_requires_it() {
        let s0 = S::new().f("a", "string", true).f("b", "string", false);
        let s1 = S::new().f("a", "string", false).f("b", "string", true);
        let req = S::new().f("a", "string", true).f("b", "string", true);
        assert_eq!(
            rules(&server_moved(R, &req, &s0, &s1, false)),
            [row(
                "a",
                REQUIRED,
                SERVER,
                "response_field_now_optional",
                true
            )],
            "optional to required is no change"
        );
        let opt = S::new().f("a", "string", false);
        let j = server_moved(R, &opt, &s0, &s1, false);
        assert_eq!(
            rules(&j),
            [row(
                "a",
                REQUIRED,
                SERVER,
                "response_field_now_optional",
                false
            )]
        );
        assert_eq!(
            (
                j.verdicts[0].before.as_deref(),
                j.verdicts[0].after.as_deref()
            ),
            (Some("required"), Some("optional"))
        );
        assert!(rules(&server_moved(R, &S::new(), &s0, &s1, false)).is_empty());
    }

    // ---- response: client-side changes -----------------------------------

    #[test]
    fn a_new_client_read_the_server_never_sends_is_missing() {
        let s = S::new().f("id", "string", true).f("sent", "string", true);
        let c0 = S::new().f("id", "string", true);
        let c1 = S::new()
            .f("id", "string", true)
            .f("trackingUrl", "string", true)
            .f("hint", "string", false)
            .f("sent", "string", true);
        let j = client_moved(R, &c0, &c1, &s);
        assert_eq!(
            rules(&j),
            [
                row("hint", ADDED, CLIENT, "json_field_optional_absent", false),
                row("trackingUrl", ADDED, CLIENT, "json_field_missing", true),
            ],
            "a new read the server sends is no change"
        );
        assert_eq!(j.verdicts[1].server, None);
        assert!(
            rules(&client_moved(R, &c1, &c0, &s)).is_empty(),
            "a dropped read is no change"
        );
    }

    #[test]
    fn a_client_kind_or_requiredness_change_is_judged_against_the_server() {
        let s = S::new()
            .f("total", "number", true)
            .f("code", "string", false);
        let c0 = S::new()
            .f("total", "number", true)
            .f("code", "string", false);
        let c1 = S::new()
            .f("total", "string", true)
            .f("code", "string", true);
        assert_eq!(
            rules(&client_moved(R, &c0, &c1, &s)),
            [
                row("code", REQUIRED, CLIENT, "json_field_may_be_absent", false),
                row("total", TYPE, CLIENT, "json_type_mismatch", true),
            ]
        );
        // Back to the server's kind: no change.
        assert!(rules(&client_moved(R, &c1, &c0, &s)).is_empty());
    }

    // ---- request ------------------------------------------------------------

    #[test]
    fn a_request_rename_drops_the_clients_value() {
        let c = S::new().f("customerName", "string", true);
        let s0 = S::new().m("customerName", "CustomerName", Some("string"), true);
        let s1 = S::new().m("customer_name", "CustomerName", Some("string"), true);
        assert_eq!(
            rules(&server_moved(Q, &c, &s0, &s1, false)),
            [row(
                "customerName",
                RENAMED,
                SERVER,
                "request_json_name_changed",
                true
            )]
        );
    }

    #[test]
    fn request_fields_added_and_removed_on_the_server() {
        let c = S::new().f("name", "string", true).f("sent", "string", true);
        let s0 = S::new()
            .f("name", "string", true)
            .f("note", "string", false);
        let s1 = S::new()
            .f("name", "string", true)
            .f("email", "string", true)
            .bind("email")
            .f("sent", "string", true)
            .bind("sent")
            .f("tag", "string", false);
        assert_eq!(
            rules(&server_moved(Q, &c, &s0, &s1, false)),
            [
                row("email", ADDED, SERVER, "new_required_request_field", true),
                row("note", REMOVED, SERVER, "request_field_removed", false),
                row("tag", ADDED, SERVER, "request_field_added", false),
            ],
            "a required bind the client already sends is no change"
        );
    }

    #[test]
    fn a_bind_made_required_or_retyped_breaks_the_clients_request() {
        let s0 = S::new().f("a", "string", true).f("b", "number", true);
        let s1 = S::new()
            .f("a", "string", true)
            .bind("a")
            .f("b", "string", true);
        let lacks = S::new().f("b", "number", true);
        let j = server_moved(Q, &lacks, &s0, &s1, false);
        assert_eq!(
            rules(&j),
            [
                row("a", REQUIRED, SERVER, "new_required_request_field", true),
                row("b", TYPE, SERVER, "request_type_changed", true),
            ]
        );
        assert_eq!(
            (
                j.verdicts[0].before.as_deref(),
                j.verdicts[0].after.as_deref()
            ),
            (Some("optional"), Some("required"))
        );
        let sends = S::new().f("a", "string", true).f("b", "string", true);
        assert!(rules(&server_moved(Q, &sends, &s0, &s1, false)).is_empty());
    }

    #[test]
    fn request_client_changes_against_the_binder() {
        let s = S::new()
            .f("name", "string", true)
            .bind("name")
            .f("note", "string", false)
            .f("qty", "number", true);
        let c0 = S::new()
            .f("name", "string", true)
            .f("note", "string", true)
            .f("qty", "number", true);
        let c1 = S::new().f("qty", "string", true).f("extra", "string", true);
        assert_eq!(
            rules(&client_moved(Q, &c0, &c1, &s)),
            [
                row("extra", ADDED, CLIENT, "json_field_dropped", false),
                row("name", REMOVED, CLIENT, "json_required_missing", true),
                row("note", REMOVED, CLIENT, "json_field_unset", false),
                row("qty", TYPE, CLIENT, "json_type_mismatch", true),
            ]
        );
    }

    #[test]
    fn a_go_binder_folds_request_names_exact_first() {
        let c = S::new()
            .f("customername", "string", true)
            .f("email", "string", true);
        let s0 = S::new()
            .f("customerName", "string", true)
            .bind("customerName");
        let s1 = S::new()
            .f("customerName", "string", true)
            .bind("customerName")
            .f("Email", "string", true)
            .bind("Email");
        assert!(
            rules(&server_moved(Q, &c, &s0, &s1, true)).is_empty(),
            "the client sends `email`, which a Go binder folds into `Email`"
        );
        assert_eq!(
            rules(&server_moved(Q, &c, &s0, &s1, false)),
            [row(
                "Email",
                ADDED,
                SERVER,
                "new_required_request_field",
                true
            )]
        );
        // A client name the binder folds is no dropped field either.
        let c0 = S::new().f("customername", "string", true);
        let c1 = S::new()
            .f("customername", "string", true)
            .f("EMAIL", "string", true);
        assert!(
            judge(Q, Some(&c0.0), &c1.0, None, &s1.0, true)
                .verdicts
                .is_empty()
        );
    }

    // ---- selection ------------------------------------------------------------

    #[test]
    fn a_selected_schema_field_removed_retyped_or_made_nullable_breaks() {
        let c = S::new()
            .k("stock", None, true)
            .k("price", None, true)
            .k("name", None, true);
        let s0 = S::new()
            .f("stock", "number", true)
            .f("price", "number", true)
            .f("name", "string", true)
            .f("sku", "string", true);
        let s1 = S::new()
            .f("price", "string", true)
            .f("name", "string", false)
            .f("quantity", "number", true);
        let j = server_moved(SEL, &c, &s0, &s1, false);
        assert_eq!(
            rules(&j),
            [
                row("name", REQUIRED, SERVER, "schema_field_now_nullable", true),
                row("price", TYPE, SERVER, "schema_field_type_changed", true),
                row("stock", REMOVED, SERVER, "schema_field_removed", true),
            ],
            "sku is selected by no one, quantity is new: neither is a change"
        );
        assert_eq!(
            (
                j.verdicts[0].before.as_deref(),
                j.verdicts[0].after.as_deref()
            ),
            (Some("non-null"), Some("nullable"))
        );
    }

    #[test]
    fn a_new_selection_the_type_lacks_is_undefined() {
        let s = S::new().f("id", "string", true);
        let c0 = S::new().k("id", None, true);
        let c1 = S::new().k("id", None, true).k("barcode", None, true);
        let j = client_moved(SEL, &c0, &c1, &s);
        assert_eq!(
            rules(&j),
            [row(
                "barcode",
                ADDED,
                CLIENT,
                "graphql_field_undefined",
                true
            )]
        );
        assert_eq!(j.verdicts[0].after.as_deref(), Some("barcode"));
    }

    // ---- common -----------------------------------------------------------------

    #[test]
    fn no_absence_is_claimed_below_an_open_path() {
        let c = S::new()
            .f("meta", "object", true)
            .f("meta.a", "string", true);
        let s0 = S::new()
            .f("meta", "object", true)
            .f("meta.a", "string", true);
        let s1 = S::new().f("meta", "object", true).open("meta");
        let j = server_moved(R, &c, &s0, &s1, false);
        assert!(j.verdicts.is_empty(), "{:?}", j.verdicts);
        assert!(j.open);
        // A client's new read below the server's open path.
        let c1 = S::new()
            .f("meta", "object", true)
            .f("meta.b", "string", true);
        let j = client_moved(R, &S::new().f("meta", "object", true), &c1, &s1);
        assert!(j.verdicts.is_empty() && j.open);
        // A new required bind the client may send through its open value.
        let c = S::new().f("body", "object", true).open("body");
        let b0 = S::new().f("body", "object", true);
        let b1 = S::new()
            .f("body", "object", true)
            .f("body.email", "string", true)
            .bind("body.email");
        let j = server_moved(Q, &c, &b0, &b1, false);
        assert!(j.verdicts.is_empty() && j.open);
    }

    #[test]
    fn only_the_topmost_of_a_removed_or_added_subtree_is_listed() {
        let c = S::new()
            .f("lines", "array", true)
            .f("lines[].productId", "string", true)
            .f("lines[].qty", "number", true);
        let s0 = S::new()
            .f("lines", "array", true)
            .f("lines[].productId", "string", true)
            .f("lines[].qty", "number", true);
        let j = server_moved(R, &c, &s0, &S::new(), false);
        assert_eq!(
            rules(&j),
            [row(
                "lines",
                REMOVED,
                SERVER,
                "response_field_removed",
                true
            )]
        );
        let s = S::new();
        let c1 = S::new()
            .f("owner", "object", true)
            .f("owner.email", "string", true);
        assert_eq!(
            rules(&client_moved(R, &S::new(), &c1, &s)),
            [row("owner", ADDED, CLIENT, "json_field_missing", true)]
        );
        // A retyped field's subtree is the type change.
        let s0 = S::new()
            .f("meta", "object", true)
            .f("meta.a", "string", true);
        let s1 = S::new().f("meta", "string", true);
        let c = S::new()
            .f("meta", "object", true)
            .f("meta.a", "string", true);
        assert_eq!(
            rules(&server_moved(R, &c, &s0, &s1, false)),
            [row("meta", TYPE, SERVER, "response_type_changed", true)]
        );
    }

    #[test]
    fn a_path_both_sides_moved_is_judged_by_the_servers_rule() {
        let c0 = S::new().f("id", "string", true).f("total", "number", true);
        let c1 = S::new().f("id", "string", true);
        let s0 = S::new().f("id", "string", true).f("total", "number", true);
        let s1 = S::new().f("id", "string", true);
        let j = judge(R, Some(&c0.0), &c1.0, Some(&s0.0), &s1.0, false);
        assert_eq!(
            rules(&j),
            [row("total", REMOVED, BOTH, "response_field_removed", false)]
        );
        assert_eq!(j.verdicts[0].client, Some(("total".to_string(), At::Base)));
    }

    #[test]
    fn verdicts_are_sorted_and_deterministic() {
        let c = S::new().f("b", "number", true).f("a", "string", true);
        let s0 = S::new().f("b", "number", true).f("a", "string", true);
        let s1 = S::new().f("z", "string", true);
        let one = rules(&server_moved(R, &c, &s0, &s1, false));
        let two = rules(&server_moved(R, &c, &s0, &s1, false));
        assert_eq!(one, two);
        let paths: Vec<&str> = one.iter().map(|r| r.0.as_str()).collect();
        assert_eq!(paths, ["a", "b", "z"]);
    }

    #[test]
    fn paths_nest_by_dot_and_element() {
        assert!(under("lines[].qty", "lines"));
        assert!(under("owner.email", "owner"));
        assert!(under("id", ""));
        assert!(!under("lines2", "lines"));
        assert!(!under("lines", "lines"));
        assert!(!under("", ""));
        assert_eq!(parent("lines[].qty"), "lines[]");
        assert_eq!(parent("id"), "");
        assert_eq!(wire_path("[].id"), "id");
        assert_eq!(wire_path("[]"), "");
        assert_eq!(wire_path("items[].sku"), "items[].sku");
        assert_eq!(display(""), "(root)");
    }

    // ---- exchanges, tiers, rows ---------------------------------------------

    fn exchange(key: &str, route: u64, handler: u64, file: &str, client_root: u64) -> Exchange {
        Exchange {
            pairing: HTTP_MODEL,
            dir: R,
            key: key.to_string(),
            route,
            handler,
            endpoint: 100 + route,
            file: file.to_string(),
            line: 1,
            client_root,
            server_root: 900,
            client_format: Some(WireFormat::Ts),
            server_format: Some(WireFormat::Go),
            client: WireFields::default(),
            server: WireFields::default(),
            fold: false,
            judgeable: true,
            statics: Vec::new(),
            client_site: ModelSite {
                file: Some(file.to_string()),
                line: Some(1),
                at_base: false,
            },
            server_site: ModelSite {
                file: Some("h.go".to_string()),
                line: Some(2),
                at_base: false,
            },
            op: None,
        }
    }

    #[test]
    fn exchanges_pair_by_ident_then_by_the_one_candidate_left_on_a_route() {
        let id = |x: u64| x;
        // Same ident and client root; a renamed handler on GET /b; a renamed
        // client model on GET /c.
        let old = [
            exchange("GET /a", 1, 10, "a.ts", 50),
            exchange("GET /b", 2, 20, "a.ts", 50),
            exchange("GET /c", 3, 30, "a.ts", 50),
        ];
        let new = [
            exchange("GET /a", 1, 10, "a.ts", 50),
            exchange("GET /b", 2, 21, "a.ts", 50),
            exchange("GET /c", 3, 30, "a.ts", 51),
        ];
        assert_eq!(pair_exchanges(&old, &new, &id), [Some(0), Some(1), Some(2)]);
        // Two candidates on one route: nothing is matched, so one is new and
        // two are lost.
        let old = [
            exchange("GET /b", 2, 20, "a.ts", 50),
            exchange("GET /b", 2, 22, "a.ts", 50),
        ];
        let new = [exchange("GET /b", 2, 21, "a.ts", 50)];
        assert_eq!(pair_exchanges(&old, &new, &id), [None]);
        // A moved id pairs through the map.
        let old = [exchange("GET /a", 7, 10, "a.ts", 50)];
        let new = [
            exchange("GET /a", 1, 10, "a.ts", 50),
            exchange("GET /z", 9, 90, "z.ts", 50),
        ];
        let map = |x: u64| {
            if x == 7 {
                1
            } else if x == 107 {
                101
            } else {
                x
            }
        };
        assert_eq!(pair_exchanges(&old, &new, &map), [Some(0), None]);
    }

    fn repo() -> RepoId {
        RepoId::from_canonical("test://model-breaks")
    }

    fn nid(kind: NodeKindId, q: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, q)
    }

    fn pos(file: &str, row: u32) -> Cell {
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":{row},"end_line":{row}}}"#
            )),
        }
    }

    fn code(text: &str) -> Cell {
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text.to_string()),
        }
    }

    fn merged(nodes: Vec<(NodeKindId, &str, Vec<Cell>)>, cross: Vec<Edge>) -> MergedGraph {
        let mut nav = CodeNav::default();
        let mut out = Vec::new();
        for (kind, q, cells) in nodes {
            let id = nid(kind, q);
            nav.record(id, q, q, kind, None);
            out.push(Node {
                id,
                repo: repo(),
                confidence: Confidence::Strong,
                cells,
            });
        }
        let g = RepoGraph {
            repo: repo(),
            nodes: out,
            edges: vec![],
            nav,
            symbols: SymbolTable::default(),
            unresolved_calls: vec![],
            unresolved_refs: vec![],
            properties: Default::default(),
        };
        let mut m = MergedGraph::new(vec![g]);
        m.cross_edges = cross;
        m
    }

    fn edge(from: u64, to: u64, cat: EdgeCategoryId, emitter: &str, rule: &str) -> Edge {
        Edge::new(NodeId(from), NodeId(to), cat, Confidence::Strong).with_cell(
            Evidence::emitter(emitter)
                .rule(rule)
                .at("x.go", 3)
                .to_cell(),
        )
    }

    #[test]
    fn an_exchange_takes_each_categorys_strongest_edge_and_the_weakest_category() {
        let ex = exchange("GET /a", 1, 10, "a.ts", 50);
        let (ep, route, h, model) = (ex.endpoint, ex.route, ex.handler, ex.server_root);
        let fact = "parser:go";
        let calls = |emitter: &str| edge(ep, route, edge_category::HTTP_CALLS, emitter, "exact");
        let handled = edge(route, h, edge_category::HANDLED_BY, fact, "type_method");
        let uses = |rule: &str| edge(h, model, edge_category::USES, fact, rule);
        let m = merged(
            vec![],
            vec![calls(fact), handled.clone(), uses("json_response")],
        );
        assert_eq!(exchange_tier(&EdgeIndex::new(&m), &ex), check::FACT);
        let m = merged(
            vec![],
            vec![
                calls("resolver:http"),
                handled.clone(),
                uses("json_response"),
            ],
        );
        assert_eq!(exchange_tier(&EdgeIndex::new(&m), &ex), check::DERIVED);
        // Two HTTP_CALLS edges: the strongest counts.
        let m = merged(
            vec![],
            vec![
                calls("resolver:http"),
                calls(fact),
                handled.clone(),
                uses("json_response"),
            ],
        );
        assert_eq!(exchange_tier(&EdgeIndex::new(&m), &ex), check::FACT);
        // A USES edge of the other direction's form joins nothing: derived.
        let m = merged(
            vec![],
            vec![calls(fact), handled.clone(), uses("json_bind")],
        );
        assert_eq!(exchange_tier(&EdgeIndex::new(&m), &ex), check::DERIVED);
        // An inline server root needs no USES edge.
        let mut inline = ex.clone();
        inline.server_root = h;
        let m = merged(vec![], vec![calls(fact), handled]);
        assert_eq!(exchange_tier(&EdgeIndex::new(&m), &inline), check::FACT);
    }

    fn change_of(rule: &'static str, breaking: bool) -> ModelChange {
        ModelChange {
            field: "total".to_string(),
            change: REMOVED,
            side: SERVER,
            rule,
            breaking,
            before: Some("number".to_string()),
            after: None,
            client: None,
            server: None,
        }
    }

    fn judged_row(tiers: &[&'static str], min_tier: &'static str, new: bool) -> ModelBreak {
        let m = merged(vec![], vec![]);
        let (loc, nodes) = (Locator::new(&m), NodeIndex::new(&m));
        let now = Rev {
            m: &m,
            loc: &loc,
            nodes: &nodes,
        };
        let exchanges: Vec<Exchange> = tiers
            .iter()
            .enumerate()
            .map(|(i, _)| exchange(&format!("GET /{i}"), i as u64, 10, "a.ts", 50))
            .collect();
        let judged: Vec<JudgedExchange> = tiers
            .iter()
            .enumerate()
            .map(|(i, t)| JudgedExchange {
                ex: i,
                new,
                tier: t,
                changes: vec![change_of("response_field_removed", true)],
                open: false,
            })
            .collect();
        let mut rows = rows_of(&exchanges, &judged, &now, min_tier);
        assert_eq!(rows.len(), 1);
        rows.remove(0)
    }

    #[test]
    fn the_gate_blocks_by_tier_and_min_tier_and_never_on_a_heuristic() {
        let gate = |t: &'static str, min: &'static str| judged_row(&[t], min, false).blocking;
        assert!(gate(check::FACT, check::FACT));
        assert!(
            !gate(check::DERIVED, check::FACT),
            "derived under fact is advisory"
        );
        assert!(gate(check::DERIVED, check::DERIVED));
        assert!(gate(check::FACT, check::DERIVED));
        assert!(!gate(check::HEURISTIC, check::DERIVED));
        assert!(!gate(check::HEURISTIC, check::FACT));
    }

    #[test]
    fn a_row_takes_its_strongest_exchanges_tier() {
        let r = judged_row(
            &[check::DERIVED, check::FACT, check::HEURISTIC],
            check::FACT,
            false,
        );
        assert_eq!((r.tier, r.blocking), (check::FACT, true));
        assert_eq!(r.exchanges.len(), 3);
        assert_eq!(
            r.exchanges
                .iter()
                .map(|e| e.key.as_str())
                .collect::<Vec<_>>(),
            ["GET /0", "GET /1", "GET /2"]
        );
        assert_eq!(
            r.changes.len(),
            1,
            "one change, deduplicated across exchanges"
        );
        assert_eq!((r.change, r.changed, r.note), ("modified", SERVER, None));
    }

    #[test]
    fn a_new_pairing_lists_its_static_changes_as_added() {
        let r = judged_row(&[check::DERIVED], check::FACT, true);
        assert_eq!(
            (r.change, r.changed, r.note, r.status),
            (ADDED, "pairing", Some(NOTE_NEW_PAIRING), "breaking")
        );
        // static_changes over a new exchange's DT.1m changes.
        let m = merged(vec![], vec![]);
        let (loc, nodes) = (Locator::new(&m), NodeIndex::new(&m));
        let rev = Rev {
            m: &m,
            loc: &loc,
            nodes: &nodes,
        };
        let mut ex = exchange("GET /a", 1, 10, "a.ts", 50);
        ex.client = S::new().f("trackingUrl", "string", true).0;
        ex.statics = vec![StaticChange {
            path: wire_path("[].trackingUrl"),
            rule: "json_field_missing",
            client: Some("string".to_string()),
            server: None,
        }];
        let moved = HashMap::new();
        let fl = FieldLocator {
            then: &rev,
            now: &rev,
            moved: &moved,
            ex: &ex,
            base: None,
        };
        let got = static_changes(&ex, &fl);
        assert_eq!(got.len(), 1);
        let c = &got[0];
        assert_eq!(
            (
                c.field.as_str(),
                c.change,
                c.side,
                c.rule,
                c.breaking,
                c.after.as_deref()
            ),
            (
                "trackingUrl",
                ADDED,
                CLIENT,
                "json_field_missing",
                true,
                Some("string")
            )
        );
        assert!(c.server.is_none());
    }

    #[test]
    fn member_lines_per_format() {
        let go = "Order struct {\n\tID           string `json:\"id\"`\n\tCustomerName string `json:\"customer_name\"`\n\tA, B int\n}";
        let go_f = Some(WireFormat::Go);
        assert_eq!(
            member_line(go, go_f, "customer_name", "CustomerName"),
            Some(2)
        );
        assert_eq!(member_line(go, go_f, "id", "ID"), Some(1));
        assert_eq!(
            member_line(go, go_f, "b", "B"),
            None,
            "a multi-name line falls back"
        );
        assert_eq!(
            member_line(go, go_f, "x", "Customer"),
            None,
            "a prefix is no member"
        );
        let ts = "interface Order {\n  readonly id: string;\n  'total-cents'?: number;\n  customerName: string;\n}";
        let ts_f = Some(WireFormat::Ts);
        assert_eq!(member_line(ts, ts_f, "id", "id"), Some(1));
        assert_eq!(member_line(ts, ts_f, "total-cents", "total-cents"), Some(2));
        assert_eq!(
            member_line(ts, ts_f, "customerName", "customerName"),
            Some(3)
        );
        assert_eq!(member_line(ts, ts_f, "customer", "customer"), None);
        let dart = "class Order {\n  final String customerName;\n  late int total;\n  Order.fromJson(Map j)\n    : customerName = j['customer_name'];\n}";
        let dart_f = Some(WireFormat::Dart);
        assert_eq!(
            member_line(dart, dart_f, "customer_name", "customerName"),
            Some(4)
        );
        assert_eq!(member_line(dart, dart_f, "total", "total"), Some(2));
        assert_eq!(member_line("type X struct { A int }", go_f, "a", "A"), None);
        assert_eq!(
            member_line("A int", go_f, "a", "A"),
            Some(0),
            "a one-line CODE is read from line 0"
        );
    }

    #[test]
    fn member_sites_fall_back_to_the_declaring_nodes_line() {
        let m = merged(
            vec![
                (
                    node_kind::STRUCT,
                    "models::Order",
                    vec![
                        pos("models/order.go", 19),
                        code("Order struct {\n\tTotal int `json:\"total\"`\n}"),
                        glia_code_domain::wire::ModelSchema::new(WireFormat::Go).cell(),
                    ],
                ),
                (
                    node_kind::STRUCT,
                    "models::Bare",
                    vec![pos("models/bare.go", 4)],
                ),
            ],
            vec![],
        );
        let (loc, nodes) = (Locator::new(&m), NodeIndex::new(&m));
        let rev = Rev {
            m: &m,
            loc: &loc,
            nodes: &nodes,
        };
        let site = |q: &str, name: &str, member: &str| {
            member_site(&rev, nid(node_kind::STRUCT, q), name, member)
                .map(|(s, hit)| (s.file.unwrap_or_default(), s.line, hit))
        };
        assert_eq!(
            site("models::Order", "total", "Total"),
            Some(("models/order.go".to_string(), Some(21), true))
        );
        assert_eq!(
            site("models::Order", "gone", "Gone"),
            Some(("models/order.go".to_string(), Some(20), false))
        );
        assert_eq!(
            site("models::Bare", "x", "X"),
            Some(("models/bare.go".to_string(), Some(5), false)),
            "no CODE: the node's own line"
        );
        assert_eq!(site("models::Missing", "x", "X"), None);
    }

    #[test]
    fn a_removed_field_is_located_now_then_at_the_rev_then_at_the_root() {
        let order = |code_text: &str| {
            merged(
                vec![(
                    node_kind::STRUCT,
                    "models::Order",
                    vec![
                        pos("models/order.go", 9),
                        code(code_text),
                        glia_code_domain::wire::ModelSchema::new(WireFormat::Go).cell(),
                    ],
                )],
                vec![],
            )
        };
        let id = nid(node_kind::STRUCT, "models::Order");
        let field = |member: &str, declared_by: Option<NodeId>| {
            let mut f = WireField::default();
            f.name = member.to_lowercase();
            f.member = member.to_string();
            f.kind = Some("string");
            f.declared_by = declared_by;
            f
        };
        let side = |fields: Vec<(&str, WireField)>| {
            let mut w = WireFields::default();
            let mut root = WireField::default();
            root.model = Some(id);
            w.fields.insert(String::new(), root);
            for (k, f) in fields {
                w.fields.insert(k.to_string(), f);
            }
            w
        };
        let mut now_ex = exchange("GET /o", 1, 10, "a.ts", 50);
        now_ex.server_root = id.0;
        now_ex.server = side(vec![("id", field("ID", Some(id)))]);
        let mut base_ex = now_ex.clone();
        base_ex.server = side(vec![
            ("id", field("ID", Some(id))),
            ("gone", field("Gone", Some(id))),
            ("lost", field("Lost", Some(NodeId(424242)))),
        ]);
        let then_m = order("Order struct {\n\tID string\n\tGone string\n}");
        let line = |now_code: &str, key: &str, at: At| {
            let now_m = order(now_code);
            let (tl, tn) = (Locator::new(&then_m), NodeIndex::new(&then_m));
            let (nl, nn) = (Locator::new(&now_m), NodeIndex::new(&now_m));
            let then = Rev {
                m: &then_m,
                loc: &tl,
                nodes: &tn,
            };
            let now = Rev {
                m: &now_m,
                loc: &nl,
                nodes: &nn,
            };
            let moved = HashMap::new();
            let fl = FieldLocator {
                then: &then,
                now: &now,
                moved: &moved,
                ex: &now_ex,
                base: Some(&base_ex),
            };
            fl.server(key, at).map(|s| (s.line, s.at_base))
        };
        let gone = "Order struct {\n\tID string\n}";
        assert_eq!(
            line(gone, "gone", At::Base),
            Some((Some(12), true)),
            "the rev's member line"
        );
        assert_eq!(
            line(
                "Order struct {\n\tID string\n\tGone string `json:\"-\"`\n}",
                "gone",
                At::Base
            ),
            Some((Some(12), false)),
            "the working tree still declares the member"
        );
        assert_eq!(
            line(gone, "lost", At::Base),
            Some((Some(10), false)),
            "a declaring node located nowhere: the working tree root's line"
        );
        assert_eq!(line(gone, "id", At::Now), Some((Some(11), false)));
        assert_eq!(
            line(gone, "", At::Now),
            Some((Some(10), false)),
            "the root: its model"
        );
    }

    #[test]
    fn selection_sites_track_brace_depth() {
        let doc = "\n  query Stock {\n    items(first: 2) {\n      owner {\n        name\n      }\n      name\n      code: barcode\n    }\n    other { stock }\n  }\n";
        let m = merged(
            vec![(
                node_kind::GRAPHQL_OPERATION,
                "graphql_op:Stock",
                vec![pos("q.ts", 2), code(doc)],
            )],
            vec![],
        );
        let (loc, nodes) = (Locator::new(&m), NodeIndex::new(&m));
        let rev = Rev {
            m: &m,
            loc: &loc,
            nodes: &nodes,
        };
        let op = nid(node_kind::GRAPHQL_OPERATION, "graphql_op:Stock").0;
        let line = |path: &[&str]| selection_site(&rev, op, path).and_then(|s| s.line);
        // The op's CODE opens on line 3 (its POSITION row 2).
        assert_eq!(line(&["items"]), Some(5));
        assert_eq!(
            line(&["items", "name"]),
            Some(9),
            "not the owner's name one level down"
        );
        assert_eq!(line(&["items", "owner", "name"]), Some(7));
        assert_eq!(
            line(&["items", "barcode"]),
            Some(10),
            "an alias reads its field"
        );
        assert_eq!(
            line(&["items", "stock"]),
            Some(3),
            "stock is other's: the op's line"
        );
        assert_eq!(line(&["missing"]), Some(3));
    }
}
