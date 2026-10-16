//! DT.1g: the models pass's inputs. [`client_wires`] collects both sides of
//! every HTTP exchange the graph already pairs and resolves each side's
//! shape to the models it carries, so a later step (DT.1m's compare, DT.1h's
//! identity, DT.1i's drift rows) judges models by the fields that travel,
//! never by their names.
//!
//! **The client side.** Every ENDPOINT_HIT cell (DT.1d's `response` /
//! `request` shapes, [`hit_shapes`]) of every ENDPOINT, unioned per endpoint
//! id across graphs and sorted by (file, line), is one [`ClientWire`]. A
//! named type in the hit's shape resolves through the symbols the store
//! persists, in the MODULE whose POSITION file is the hit's file
//! ([`bind_client_name`]): that module's own `module_symbols`, then its
//! `module_import_bindings` (TS re-export barrels included, DT.1q), then for
//! `ns.Name` the namespace binding's `module_symbols`. The name is kept only
//! when it binds an INTERFACE / CLASS carrying SCHEMA_FIELDS of format `ts` /
//! `dart` ([`Ctx::resolve_client_type`]). There is no name-only fallback:
//! a name the file does not declare or import resolves to nothing.
//!
//! **The server side.** Each wire's routes are the HTTP_CALLS edges the
//! `http` resolver drew from its endpoint (EVIDENCE emitter `resolver:http`;
//! an overlay `[[edge]]` lands in Post and is not read, so the result does
//! not depend on when it is asked). The `suffix` tier is the weakest
//! pairing the resolver emits and is no ground for identity: such a pair is
//! skipped and counted `weak_pairing`. A route's handlers are its HANDLED_BY
//! targets, sorted by qname, and a handler's [`ServerSite`]s are its
//! [`ServerWire`] MESSAGE_TYPE cells (DT.1e): for a response, the
//! json_response sites whose status is known and 2xx ([`ServerWire::known_2xx`]);
//! a json_response with no `status` (a variable, a call: Kina's are error
//! bodies) is never used and is counted `server_unknown_status`; for a
//! request, every json_bind site (a bind carries no status). A site's value
//! resolves through the `model` qname the Go build wrote into its shape
//! (DT.1f); a site without one keeps its object keys or stays open.
//!
//! **Models.** A resolved model's fields ([`Ctx::model_fields`]) are the
//! union of its SCHEMA_FIELDS cells (TS declaration merging leaves one per
//! declaration), keyed by JSON name, plus the fields of the bases its
//! schema's `extends` names, reached over its INHERITS_FROM edges by simple
//! name (cycle-safe, at most [`MAX_EXTENDS`] bases deep), its own names
//! shadowing. Only `extends` is followed: a Go tagged or `json:"-"` embed
//! still draws INHERITS_FROM but its schema lists it as a field or not at
//! all. A Go INTERFACE base (an embedded interface) is a field named by its
//! type name, as encoding/json writes it. The model is **open** (fields may
//! travel that it does not list) when its schema is open, a base lacks a
//! schema or binds to nothing (an external embed such as `gorm.Model`), or,
//! for a Go model, it or a flattened base has a `MarshalJSON` method (a
//! response) or an `UnmarshalJSON` method (a request) in
//! `symbols.class_methods`: a custom codec writes whatever it likes
//! (quokka's `GeoCoordinates`). A field's value resolves the same way, its
//! model from the Go build's `model` qname or, for a TS / Dart model, its
//! element type bound in the model's own module; models nest at most
//! [`MAX_DEPTH`] levels, a model past that is listed by id, open, unexpanded.
//!
//! Merge-safe: every input is persisted (ENDPOINT_HIT / MESSAGE_TYPE /
//! SCHEMA_FIELDS cells, the symbol tables), and HTTP_CALLS is a `resolver:*`
//! cross edge a layout merge drops and re-runs (engine/src/merge.rs).
//! Deterministic: every list is sorted by content (route qname, hit file,
//! line, handler qname, site line), and maps are only probed.
//!
//! fired_on, printed by the callers ([`ClientWires::marker`]; the engine test
//! and the models resolver, [`ModelResolver`]):
//! `[models] client_wires hits=<h> routed=<r> response=<a> request=<q>
//! client_models=<m> unresolved_client=<u> server_sites=<s>
//! server_unknown_status=<k> open=<o> weak_pairing=<w>`.
//!
//! **DT.1m: the compare.** [`wire_pairs`] turns every (call site, route,
//! handler, direction) with a side on both ends into a [`WirePair`]. The
//! server side ([`pick_server`]): of a handler's distinct known-2xx response
//! shapes the one, or of several the one whose top-level names overlap the
//! client's most (a tie: note `ambiguous_server`, nothing compared); of its
//! binds exactly one distinct, else `ambiguous_server`. Names match exactly,
//! and for a request into a Go server also ASCII case-insensitively
//! (encoding/json's decode folding, an exact match first). A response
//! (the server produces): a client-only name is `json_field_missing`
//! (breaking) when required, else `json_field_optional_absent`; a
//! server-only name `json_field_unread`; a name the client requires and the
//! server may omit (omitempty) `json_field_may_be_absent`. A request (the
//! client produces): a client-only name is `json_field_dropped`; a
//! server-only name `json_required_missing` (breaking) when the binder
//! requires it, else `json_field_unset`; a name the binder requires and the
//! client marks optional `json_required_may_be_unset` (breaking: a body
//! without it is a 400; DT.1n closed this gap). Both: JSON kinds that differ while
//! both are known, array against scalar included, are `json_type_mismatch`
//! (breaking); an unknown name is an object only when it resolved to a
//! model, else (like any / null, a call no builder typed, an open value)
//! it is never judged. No name is claimed absent from an open side (note
//! `open`); a union whose members are all objects is compared as their
//! merged fields (note `union`), and a client union of an envelope and a
//! bare array (`{ activities: C[] } | C[]`, a client accepting both shapes)
//! is compared by its member of the server value's kind (DT.1l's rule,
//! [`union_member`]: a union matches when one member does). Paths flatten
//! as `items[].sku`. Arrays and
//! same-named fields recurse to [`MAX_DEPTH`], under inline roots too
//! (`{ listings: Listing[] }` against `gin.H{"listings": ..}`). Where both
//! sides are models with listed fields and [`confirms`] holds (at least one
//! shared name, and at least half of the smaller side's) the two are a
//! [`ModelPair`], diverged when a change sits at that model's level (a
//! nested model level owns its own). Read by DT.1h (identity), DT.1i (drift
//! rows in contract_fields).
//!
//! fired_on ([`WirePairs::marker`], printed by the engine test, by the
//! models resolver ([`ModelResolver`]) and by contract_fields from DT.1i):
//! `[models] pairs=<p> response=<r> request=<q> model_pairs=<m> changes=<c>
//! breaking=<b> ambiguous_server=<a> open=<o>`.
//!
//! **DT.1h: identity.** [`ModelResolver`], the `models` Resolve pass, turns
//! the confirmed model pairs into `http_model` SAME_AS classes, one star per
//! server model (the center) whose client models it alone pairs: a client
//! that pairs a struct and its embedder keeps the embedded base, one that
//! pairs two unrelated structs joins nothing, and two client models of one
//! project paired with one struct are both dropped ([`ModelResolver`] has the
//! rules and its marker, `[models] classes=..`).
//!
//! **DT.1n: GraphQL selection drift.** [`graphql_pairs`] reads every
//! GRAPHQL_CALLS op -> resolver pair the `graphql` resolver drew (DT.11a:
//! by the root fields the op's document selects) and compares the op's
//! selection (DT.1j's `graphql_selection` MESSAGE_TYPE cell) under that root
//! field with the SDL type the root field returns (its `graphql_returns`
//! cell names `T`; the type is the MESSAGE_TYPE `message:graphql:<T>` plus
//! the resolver's owner segment, in the resolver's repo). A selected field
//! the type does not declare is `graphql_field_undefined` (breaking: the
//! server rejects the document). Object-typed fields with a sub-selection
//! recurse into the type their declared type names. A schema field the
//! selection does not ask for is no change: a selection is a projection by
//! design. Nothing is claimed absent from an open side (note `open`): an
//! extend-only type or one with a template hole, an interface type, a
//! selection with an inline fragment or an unknown spread, a sub-selected
//! field whose type names no MESSAGE_TYPE (a union, a type outside the
//! build). A pair is refused (nothing compared, the reason kept) on
//! conflicting copies (two selections, return types or field lists of one
//! node), on a type that exists only under another owner, or when no root
//! selection names the resolver's field. Variables and input types are not
//! compared. See [`GraphqlPairs::marker`] for the fired_on line, printed by
//! contract_fields (DT.1n's `graphql_selection` rows) and the engine test.
//!
//! **DT.1k: graphql_model identity.** The same pass joins a GraphQL client
//! call's TS result model and gqlgen's generated Go model with the SDL type
//! the call's root field returns, in one `graphql_model` SAME_AS class
//! centred on the type's MESSAGE_TYPE: the TS parser's `graphql_result`
//! MESSAGE_TYPE cell names the call's document and result type, the
//! document binds to its `gql` STATE_VAR, the op minted in that const's span
//! is the GRAPHQL_CALLS side, each result key's root field names the type,
//! and field names confirm both members (see [`graphql_model_identity_in`]).
//!
//! **DT.1o: one side flattened.** [`wire_fields`] lists one HTTP side (a
//! call site's resolved shape, a handler site's) as JSON paths, and
//! [`graphql_fields`] a GraphQL pair's selection and the schema type it is
//! served from, by schema-field path. Each entry keeps its member, kind,
//! requiredness and declaring node; the paths below which a name may travel
//! unlisted are `open`. The engine's `contract_breaks::model_breaks` judges a
//! side at a git rev against the same side in the working tree through them.

use std::borrow::Cow;
use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_activation::profile::{IdentityRule, IdentityTier};
use glia_code_domain::endpoint::{http_node_span, split_owner};
use glia_code_domain::evidence::{Basis, Evidence, Location, locate};
use glia_code_domain::profile::CODE_TABLES;
use glia_code_domain::same_as;
use glia_code_domain::wire::{
    JsonKind, ModelField, ModelSchema, ServerWire, Shape, ShapeField, WireForm, WireFormat,
    element_type, hit_shapes, json_kind,
};
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_code_extractors::generated::{Generated, generated_by_banner, generated_by_name};
use glia_core::{Cell, CellPayload, Confidence, Edge, NodeId, NodeKindId, RepoId};

use super::host::str_field;
use super::{CrossGraphResolver, RuleTally, rule_evidence};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

/// How many levels of models and inline objects a shape expands: the root is
/// level 0, a model met at level `MAX_DEPTH` is listed by id, open and
/// unexpanded.
pub(crate) const MAX_DEPTH: usize = 4;

/// How many `extends` bases deep a model flattens.
pub(crate) const MAX_EXTENDS: usize = 4;

/// The emitter of the HTTP_CALLS edges a wire's routes are read from.
const HTTP_EMITTER: &str = "resolver:http";

/// The HTTP tier that is no pairing to build identity on
/// (graph/src/resolvers/http.rs `MatchTier::Suffix`).
const WEAK_RULE: &str = "suffix";

/// The node kinds a model can be: a client's INTERFACE / CLASS, a server's
/// STRUCT (or a CLASS / INTERFACE in a language whose server sites are typed
/// later).
const MODEL_KINDS: [NodeKindId; 3] = [node_kind::INTERFACE, node_kind::CLASS, node_kind::STRUCT];

// ---- public types -----------------------------------------------------------

/// Which way a value travels in an HTTP exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum WireDirection {
    /// The server writes, the client reads (the call's response type).
    Response,
    /// The client writes, the server binds (the call's body).
    Request,
}

impl WireDirection {
    /// Both directions, response first.
    pub const ALL: [WireDirection; 2] = [WireDirection::Response, WireDirection::Request];

    /// `response` / `request`.
    pub fn as_str(self) -> &'static str {
        match self {
            WireDirection::Response => "response",
            WireDirection::Request => "request",
        }
    }

    /// The server-site form that carries this direction.
    fn server_form(self) -> WireForm {
        match self {
            WireDirection::Response => WireForm::JsonResponse,
            WireDirection::Request => WireForm::JsonBind,
        }
    }

    /// The Go method that replaces encoding/json's field rules for this
    /// direction on the server's model.
    fn go_codec(self) -> &'static str {
        match self {
            WireDirection::Response => "MarshalJSON",
            WireDirection::Request => "UnmarshalJSON",
        }
    }
}

/// One side's value, resolved to the models it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedShape {
    /// The shape as the call site or the schema wrote it.
    pub written: Shape,
    /// The type grammar of the written text: the source file's language (a
    /// model field: its schema's format). `None` when the file is no
    /// language the wire vocabulary reads.
    pub format: Option<WireFormat>,
    /// The in-repo model the value resolved to.
    pub model: Option<NodeId>,
    /// The value is an array of its element (one layer).
    pub array: bool,
    /// What travels under each JSON name: the model's flattened fields, or
    /// an inline object's keys. Empty for a scalar, a union, an unresolved
    /// name and a model past `MAX_DEPTH` (4) levels.
    pub fields: BTreeMap<String, ResolvedField>,
    /// A union's members, each resolved, in source order; empty otherwise.
    pub members: Vec<ResolvedShape>,
    /// Names may travel that `fields` does not list: an open expression,
    /// an open object or model, a model with a custom codec, a model name
    /// that resolved to nothing, a model past `MAX_DEPTH`, a union with an
    /// open member.
    pub open: bool,
}

/// One JSON name of a [`ResolvedShape`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ResolvedField {
    /// The JSON name.
    pub name: String,
    /// The source member when it differs from `name` (a Go json tag rename,
    /// a GraphQL alias's field).
    pub member: Option<String>,
    /// The declared type text, as written, of a model's field; `None` for an
    /// inline object's key.
    pub declared: Option<String>,
    /// Always present on the wire.
    pub required: bool,
    /// A request binder rejects a body without it.
    pub bind_required: bool,
    /// The model whose schema lists the field: the model itself or a
    /// flattened base. `None` for an inline object's key.
    pub declared_in: Option<NodeId>,
    /// The value under the name.
    pub value: ResolvedShape,
}

/// One server-side site of a handler, in one direction.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ServerSite {
    /// The handler that writes or binds the value.
    pub handler: NodeId,
    /// json_response or json_bind.
    pub form: WireForm,
    /// The site's 1-based line.
    pub line: u32,
    /// The response status (always 2xx); `None` for a bind.
    pub status: Option<u16>,
    /// The value, resolved.
    pub shape: ResolvedShape,
}

/// A route's handler and its server sites per direction.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RouteHandler {
    pub handler: NodeId,
    pub qname: String,
    /// Known-2xx json_response sites, sorted by line.
    pub response: Vec<ServerSite>,
    /// json_bind sites, sorted by line.
    pub request: Vec<ServerSite>,
}

/// One route a hit's endpoint is paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RouteSide {
    pub route: NodeId,
    pub route_qname: String,
    /// The HTTP tier the pairing's EVIDENCE names (`exact`, `base_fold`, ..;
    /// never `suffix`). Empty when the edge carries no evidence.
    pub rule: String,
    /// The route's HANDLED_BY targets, sorted by qname.
    pub handlers: Vec<RouteHandler>,
}

/// One client call site of an ENDPOINT, with its resolved shapes and the
/// server side of every route it is paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClientWire {
    pub endpoint: NodeId,
    pub endpoint_qname: String,
    /// The endpoint's LB.8 owner segment (`@services/web`), if any.
    pub owner: Option<String>,
    /// The call site's repo-relative file.
    pub file: String,
    /// The call site's 1-based line (0 when the hit records none).
    pub line: u32,
    /// The client's type grammar, read off the file's extension.
    pub format: Option<WireFormat>,
    /// The value the call reads.
    pub response: Option<ResolvedShape>,
    /// The body the call sends.
    pub request: Option<ResolvedShape>,
    /// Every non-`suffix` HTTP_CALLS route of the endpoint, sorted by qname.
    pub routes: Vec<RouteSide>,
}

impl ClientWire {
    /// The client's shape in `dir`.
    pub fn client(&self, dir: WireDirection) -> Option<&ResolvedShape> {
        match dir {
            WireDirection::Response => self.response.as_ref(),
            WireDirection::Request => self.request.as_ref(),
        }
    }
}

impl RouteHandler {
    /// The handler's sites in `dir`.
    pub fn sites(&self, dir: WireDirection) -> &[ServerSite] {
        match dir {
            WireDirection::Response => &self.response,
            WireDirection::Request => &self.request,
        }
    }
}

/// The counts behind [`ClientWires::marker`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClientWireStats {
    /// Call sites with a response or a request shape (one wire each).
    pub hits: usize,
    /// Of `hits`: paired with at least one non-`suffix` route.
    pub routed: usize,
    /// Of `hits`: with a response shape.
    pub response: usize,
    /// Of `hits`: with a request shape.
    pub request: usize,
    /// Distinct client models the hits' own shapes name (root, union
    /// member, inline key).
    pub client_models: usize,
    /// Hit directions whose root names a type (directly or as a union
    /// member) that is no scalar and binds no client model. An inline key's
    /// name that binds nothing is left unjudged and not counted: a TS type
    /// alias (`status: InvoiceStatus`) mints no node.
    pub unresolved_client: usize,
    /// Distinct handler sites returned, over every route reached.
    pub server_sites: usize,
    /// Distinct json_response sites with no status on those handlers: never
    /// returned.
    pub server_unknown_status: usize,
    /// Root shapes that are open: each wire's client side per direction and
    /// each distinct server site.
    pub open: usize,
    /// `suffix`-tier HTTP_CALLS pairs from a shaped hit's endpoint, skipped.
    pub weak_pairing: usize,
}

/// Every [`ClientWire`] of a merged graph, sorted, with its counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClientWires {
    pub wires: Vec<ClientWire>,
    pub stats: ClientWireStats,
}

impl ClientWires {
    /// The fired_on line.
    pub fn marker(&self) -> String {
        let s = &self.stats;
        format!(
            "[models] client_wires hits={} routed={} response={} request={} client_models={} \
             unresolved_client={} server_sites={} server_unknown_status={} open={} weak_pairing={}",
            s.hits,
            s.routed,
            s.response,
            s.request,
            s.client_models,
            s.unresolved_client,
            s.server_sites,
            s.server_unknown_status,
            s.open,
            s.weak_pairing,
        )
    }
}

/// Every ENDPOINT hit's response / request shape resolved to client models,
/// with the server side of each route the `http` resolver paired its
/// endpoint with (see the module doc).
pub fn client_wires(merged: &MergedGraph) -> ClientWires {
    let hits = shaped_hits(merged);
    if hits.is_empty() {
        return ClientWires::default();
    }
    let ctx = Ctx::new(merged);
    wires_in(merged, &ctx, hits)
}

/// [`client_wires`] over `hits` (never empty) with a built [`Ctx`], so the
/// models resolver resolves wires and reads model bases through one index.
fn wires_in(merged: &MergedGraph, ctx: &Ctx<'_>, hits: Vec<Hit>) -> ClientWires {
    let mut stats = ClientWireStats::default();

    // HTTP_CALLS (endpoint, route) pairs, strongest kept, `suffix` skipped.
    let shaped: BTreeSet<u64> = hits.iter().map(|h| h.endpoint.0).collect();
    let mut pairs: BTreeMap<(u64, u64), (NodeId, String)> = BTreeMap::new();
    let mut weak: BTreeSet<(u64, u64)> = BTreeSet::new();
    for e in merged
        .cross_edges
        .iter()
        .filter(|e| e.category == edge_category::HTTP_CALLS)
    {
        let ev = Evidence::of(e);
        if ev.as_ref().is_some_and(|ev| ev.emitter != HTTP_EMITTER) {
            continue;
        }
        let rule = ev.and_then(|ev| ev.rule).unwrap_or_default();
        let key = (e.from.0, e.to.0);
        if rule == WEAK_RULE {
            weak.insert(key);
        } else {
            pairs.entry(key).or_insert((e.to, rule));
        }
    }
    stats.weak_pairing = weak
        .iter()
        .filter(|k| shaped.contains(&k.0) && !pairs.contains_key(k))
        .count();
    let mut routes_of: HashMap<NodeId, Vec<(NodeId, String)>> = HashMap::new();
    for ((from, _), (to, rule)) in &pairs {
        routes_of
            .entry(NodeId(*from))
            .or_default()
            .push((*to, rule.clone()));
    }

    let mut sides: HashMap<NodeId, RouteSide> = HashMap::new();
    let mut handlers: HashMap<NodeId, RouteHandler> = HashMap::new();
    let mut client_models: BTreeSet<u64> = BTreeSet::new();
    let mut wires = Vec::with_capacity(hits.len());
    for h in hits {
        let format = format_for_file(&h.file);
        let module = ctx.hit_module(h.graph, h.endpoint, &h.file);
        let names = Names::Client(module);
        let gi = module.map_or(h.graph, |(gi, _)| gi);
        let resolve = |s: &Shape, dir| ctx.resolve(gi, names, s, format, dir, 0);
        let response = h
            .response
            .as_ref()
            .map(|s| resolve(s, WireDirection::Response));
        let request = h
            .request
            .as_ref()
            .map(|s| resolve(s, WireDirection::Request));
        for side in [&response, &request].into_iter().flatten() {
            hit_models(side, &mut client_models);
            stats.unresolved_client += root_unresolved(side);
            stats.open += usize::from(side.open);
        }
        let mut routes = Vec::new();
        for (route, rule) in routes_of.get(&h.endpoint).into_iter().flatten() {
            let side = sides
                .entry(*route)
                .or_insert_with(|| ctx.route_side(*route, rule, &mut handlers, &mut stats));
            routes.push(side.clone());
        }
        routes.sort_by(|a, b| a.route_qname.cmp(&b.route_qname));
        let qname = ctx.qname(h.graph, h.endpoint).to_string();
        let owner = split_owner(&qname).1.map(str::to_string);
        wires.push(ClientWire {
            endpoint: h.endpoint,
            endpoint_qname: qname,
            owner,
            file: h.file,
            line: h.line,
            format,
            response,
            request,
            routes,
        });
    }
    wires.sort_by(|a, b| {
        let first = |w: &ClientWire| w.routes.first().map(|r| r.route_qname.clone());
        (
            a.routes.is_empty(),
            first(a),
            &a.file,
            a.line,
            &a.endpoint_qname,
        )
            .cmp(&(
                b.routes.is_empty(),
                first(b),
                &b.file,
                b.line,
                &b.endpoint_qname,
            ))
    });
    stats.hits = wires.len();
    stats.routed = wires.iter().filter(|w| !w.routes.is_empty()).count();
    stats.response = wires.iter().filter(|w| w.response.is_some()).count();
    stats.request = wires.iter().filter(|w| w.request.is_some()).count();
    stats.client_models = client_models.len();
    ClientWires { wires, stats }
}

// ---- name binding -----------------------------------------------------------

/// The node `name` names in `module` of `g`: the module's own
/// `module_symbols`, then its `module_import_bindings`, then for `ns.Name`
/// the namespace binding's `module_symbols[Name]`. Type arguments are
/// dropped (`Page<Order>` binds `Page`). Any kind; no name-only fallback.
/// DT.1k binds a GraphQL document identifier through the same steps.
pub(crate) fn bind_client_name(g: &RepoGraph, module: NodeId, name: &str) -> Option<NodeId> {
    let name = name.split('<').next().unwrap_or(name).trim();
    if name.is_empty() {
        return None;
    }
    let own = |m: NodeId, n: &str| {
        g.symbols
            .module_symbols
            .get(&m)
            .and_then(|t| t.get(n))
            .copied()
    };
    let imported = |n: &str| {
        g.symbols
            .module_import_bindings
            .get(&module)
            .and_then(|t| t.get(n))
            .copied()
    };
    if let Some(id) = own(module, name).or_else(|| imported(name)) {
        return Some(id);
    }
    let (ns, last) = name.rsplit_once('.')?;
    let target = imported(ns.trim())?;
    own(target, last.trim())
}

/// The wire format a source file's extension names.
fn format_for_file(file: &str) -> Option<WireFormat> {
    let ext = file.rsplit_once('.').map(|(_, e)| e)?;
    match ext {
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" | "vue" | "svelte" => {
            Some(WireFormat::Ts)
        }
        "dart" => Some(WireFormat::Dart),
        "go" => Some(WireFormat::Go),
        "graphql" | "graphqls" | "gql" => Some(WireFormat::Graphql),
        _ => None,
    }
}

/// The JSON kind of `ty` in `format`; an unknown format reads every type as
/// an unknown name.
fn kind_of(format: Option<WireFormat>, ty: &str) -> JsonKind {
    format.map_or_else(|| JsonKind::Unknown(ty.to_string()), |f| json_kind(f, ty))
}

/// A base name as an `extends` entry writes it, reduced to the simple name
/// its INHERITS_FROM target carries: pointer, type arguments and qualifier
/// dropped (`*store.Base` -> `Base`).
fn simple_base(e: &str) -> &str {
    let e = e.trim().trim_start_matches('*').trim();
    let e = e.split('<').next().unwrap_or(e).trim();
    let e = e.rsplit("::").next().unwrap_or(e);
    e.rsplit('.').next().unwrap_or(e).trim()
}

// ---- hits -------------------------------------------------------------------

/// One shaped ENDPOINT_HIT: the graph its cell was read from.
struct Hit {
    graph: usize,
    endpoint: NodeId,
    file: String,
    line: u32,
    response: Option<Shape>,
    request: Option<Shape>,
}

/// Every ENDPOINT_HIT carrying a response or a request shape, unioned per
/// endpoint id across graphs (an identical payload once), endpoints in
/// first-seen order and each endpoint's hits sorted by (file, line).
fn shaped_hits(m: &MergedGraph) -> Vec<Hit> {
    let mut order: Vec<NodeId> = Vec::new();
    let mut by_endpoint: HashMap<NodeId, (BTreeSet<&str>, Vec<Hit>)> = HashMap::new();
    for (gi, g) in m.graphs.iter().enumerate() {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::ENDPOINT) {
                continue;
            }
            for c in n.cells.iter().filter(|c| c.kind == cell_type::ENDPOINT_HIT) {
                let (CellPayload::Json(p) | CellPayload::Text(p)) = &c.payload else {
                    continue;
                };
                let (response, request) = hit_shapes(p);
                if response.is_none() && request.is_none() {
                    continue;
                }
                let entry = by_endpoint.entry(n.id).or_insert_with(|| {
                    order.push(n.id);
                    (BTreeSet::new(), Vec::new())
                });
                if !entry.0.insert(p.as_str()) {
                    continue;
                }
                let (file, line0) = http_node_span(std::slice::from_ref(c)).unwrap_or_default();
                let line = line0.and_then(|l| u32::try_from(l + 1).ok()).unwrap_or(0);
                entry.1.push(Hit {
                    graph: gi,
                    endpoint: n.id,
                    file,
                    line,
                    response,
                    request,
                });
            }
        }
    }
    let mut out = Vec::new();
    for id in order {
        if let Some((_, mut hits)) = by_endpoint.remove(&id) {
            hits.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
            out.extend(hits);
        }
    }
    out
}

/// Model ids a client shape names at its own positions (root, union
/// members, inline keys), never inside a model's fields.
fn hit_models(s: &ResolvedShape, out: &mut BTreeSet<u64>) {
    if let Some(m) = s.model {
        out.insert(m.0);
        return;
    }
    for member in &s.members {
        hit_models(member, out);
    }
    for f in s.fields.values().filter(|f| f.declared_in.is_none()) {
        hit_models(&f.value, out);
    }
}

/// 1 per root name (the root, or a root union's member) that is no scalar
/// and bound no client model. TS `void` / `never` (a call that reads no
/// body) name no model.
fn root_unresolved(s: &ResolvedShape) -> usize {
    match &s.written {
        Shape::Union(_) => s.members.iter().map(root_unresolved).sum(),
        Shape::Named { ty, .. } if s.model.is_none() => usize::from(
            matches!(kind_of(s.format, ty), JsonKind::Unknown(_))
                && !(s.format == Some(WireFormat::Ts) && matches!(ty.as_str(), "void" | "never")),
        ),
        _ => 0,
    }
}

// ---- the index --------------------------------------------------------------

/// How a named type without a written `model` binds.
#[derive(Debug, Clone, Copy)]
enum Names {
    /// Through the client symbols of this (graph, MODULE); `None` when the
    /// file has no MODULE (nothing binds).
    Client(Option<(usize, NodeId)>),
    /// Only through the `model` qname a builder wrote (a server site).
    Written,
}

/// One graph's lookups, built once per [`client_wires`] call; maps are
/// only probed.
struct GraphIndex<'m> {
    g: &'m RepoGraph,
    /// Node id -> its first index in `g.nodes`.
    at: HashMap<NodeId, usize>,
    /// Every SCHEMA_FIELDS cell of a node, unioned: fields first by JSON
    /// name, `extends` deduplicated in order, open if any is.
    schemas: HashMap<NodeId, ModelSchema>,
    /// Model-kind node qname -> its ids, in `g.nodes` order.
    by_qname: HashMap<&'m str, Vec<NodeId>>,
    inherits: HashMap<NodeId, Vec<NodeId>>,
    handled_by: HashMap<NodeId, Vec<NodeId>>,
    /// POSITION file -> the least-qname MODULE declared in it.
    module_by_file: HashMap<String, NodeId>,
    /// Node id -> its POSITION file.
    file_of: HashMap<NodeId, String>,
}

impl<'m> GraphIndex<'m> {
    fn new(g: &'m RepoGraph) -> Self {
        let mut ix = GraphIndex {
            g,
            at: HashMap::new(),
            schemas: HashMap::new(),
            by_qname: HashMap::new(),
            inherits: HashMap::new(),
            handled_by: HashMap::new(),
            module_by_file: HashMap::new(),
            file_of: HashMap::new(),
        };
        for (i, n) in g.nodes.iter().enumerate() {
            ix.at.entry(n.id).or_insert(i);
            let kind = g.nav.kind_by_id.get(&n.id).copied();
            for schema in n.cells.iter().filter_map(ModelSchema::from_cell) {
                match ix.schemas.get_mut(&n.id) {
                    None => {
                        ix.schemas.insert(n.id, schema);
                    }
                    Some(have) => union_schema(have, schema),
                }
            }
            let qname = g.nav.qname_by_id.get(&n.id).map(String::as_str);
            if let (Some(k), Some(q)) = (kind, qname)
                && MODEL_KINDS.contains(&k)
            {
                let ids = ix.by_qname.entry(q).or_default();
                if !ids.contains(&n.id) {
                    ids.push(n.id);
                }
            }
            if let Some((file, _)) = locate(&n.cells) {
                if kind == Some(node_kind::MODULE) {
                    let keep = match ix.module_by_file.get(&file) {
                        None => true,
                        Some(have) => {
                            qname.unwrap_or("")
                                < g.nav.qname_by_id.get(have).map_or("", String::as_str)
                        }
                    };
                    if keep {
                        ix.module_by_file.insert(file.clone(), n.id);
                    }
                }
                ix.file_of.entry(n.id).or_insert(file);
            }
        }
        for e in &g.edges {
            let map = if e.category == edge_category::INHERITS_FROM {
                &mut ix.inherits
            } else if e.category == edge_category::HANDLED_BY {
                &mut ix.handled_by
            } else {
                continue;
            };
            let to = map.entry(e.from).or_default();
            if !to.contains(&e.to) {
                to.push(e.to);
            }
        }
        ix
    }

    fn kind(&self, id: NodeId) -> Option<NodeKindId> {
        self.g.nav.kind_by_id.get(&id).copied()
    }

    fn name(&self, id: NodeId) -> &str {
        self.g.nav.name_by_id.get(&id).map_or("", String::as_str)
    }

    fn qname(&self, id: NodeId) -> &str {
        self.g.nav.qname_by_id.get(&id).map_or("", String::as_str)
    }
}

/// `more`'s fields, bases and openness added to `have` (TS declaration
/// merging: one cell per declaration on one INTERFACE id).
fn union_schema(have: &mut ModelSchema, more: ModelSchema) {
    for f in more.fields {
        if have.field(&f.name).is_none() {
            have.fields.push(f);
        }
    }
    for e in more.extends {
        if !have.extends.contains(&e) {
            have.extends.push(e);
        }
    }
    have.open |= more.open;
}

/// What an `extends` entry reached.
enum Base {
    /// A model whose fields flatten in.
    Model(usize, NodeId),
    /// A Go embedded INTERFACE: one field named by the type.
    Field(String),
    /// No single INHERITS_FROM target of that name: external or ambiguous.
    Unbound,
}

/// The fields of one model, as [`Ctx::model_fields`] returns them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ResolvedFields {
    pub(crate) fields: BTreeMap<String, ResolvedField>,
    pub(crate) open: bool,
}

/// Every graph's [`GraphIndex`] plus the id -> graphs map; the models
/// pass's resolution context. DT.1m / DT.1k build on it.
pub(crate) struct Ctx<'m> {
    idx: Vec<GraphIndex<'m>>,
    /// Node id -> every graph holding it, in merged order.
    graphs_of: HashMap<NodeId, Vec<usize>>,
    /// `model_fields` results by (graph, model, direction, depth).
    memo: RefCell<HashMap<(usize, u64, WireDirection, usize), ResolvedFields>>,
}

impl<'m> Ctx<'m> {
    pub(crate) fn new(m: &'m MergedGraph) -> Self {
        let mut graphs_of: HashMap<NodeId, Vec<usize>> = HashMap::new();
        for (gi, g) in m.graphs.iter().enumerate() {
            for n in &g.nodes {
                let at = graphs_of.entry(n.id).or_default();
                if at.last() != Some(&gi) {
                    at.push(gi);
                }
            }
        }
        Ctx {
            idx: m.graphs.iter().map(GraphIndex::new).collect(),
            graphs_of,
            memo: RefCell::new(HashMap::new()),
        }
    }

    fn graphs(&self, id: NodeId) -> &[usize] {
        self.graphs_of.get(&id).map_or(&[], Vec::as_slice)
    }

    /// `prefer` when it holds `id`, else the first graph that does.
    fn graph_of(&self, id: NodeId, prefer: usize) -> Option<usize> {
        let at = self.graphs(id);
        if at.contains(&prefer) {
            Some(prefer)
        } else {
            at.first().copied()
        }
    }

    fn qname(&self, gi: usize, id: NodeId) -> &str {
        self.idx.get(gi).map_or("", |ix| ix.qname(id))
    }

    /// The (graph, MODULE) a hit's file declares: in the hit's own graph
    /// first, then in every other graph holding the endpoint.
    fn hit_module(&self, gi: usize, endpoint: NodeId, file: &str) -> Option<(usize, NodeId)> {
        std::iter::once(gi)
            .chain(self.graphs(endpoint).iter().copied())
            .find_map(|g| self.idx.get(g)?.module_by_file.get(file).map(|m| (g, *m)))
    }

    /// The MODULE enclosing `id` in graph `gi` (its nav parent chain).
    fn module_of(&self, gi: usize, id: NodeId) -> Option<NodeId> {
        let ix = self.idx.get(gi)?;
        let mut at = id;
        for _ in 0..64 {
            at = *ix.g.nav.parent_of.get(&at)?;
            if ix.kind(at) == Some(node_kind::MODULE) {
                return Some(at);
            }
        }
        None
    }

    /// The client model `name` names in `module`: [`bind_client_name`],
    /// kept only for an INTERFACE / CLASS carrying SCHEMA_FIELDS of format
    /// `ts` / `dart`.
    pub(crate) fn resolve_client_type(
        &self,
        gi: usize,
        module: NodeId,
        name: &str,
    ) -> Option<(usize, NodeId)> {
        let ix = self.idx.get(gi)?;
        let id = bind_client_name(ix.g, module, name)?;
        let at = self.graph_of(id, gi)?;
        let target = &self.idx[at];
        let kind_ok = matches!(target.kind(id), Some(k) if k == node_kind::INTERFACE || k == node_kind::CLASS);
        let format_ok = target
            .schemas
            .get(&id)
            .is_some_and(|s| matches!(s.format, WireFormat::Ts | WireFormat::Dart));
        (kind_ok && format_ok).then_some((at, id))
    }

    /// The model a builder-written `model` qname names: a model-kind node of
    /// that qname in graph `gi` (a node carrying a schema first), else in
    /// any graph.
    fn model_by_qname(&self, gi: usize, qname: &str) -> Option<(usize, NodeId)> {
        let pick = |g: usize| {
            let ix = self.idx.get(g)?;
            let ids = ix.by_qname.get(qname)?;
            ids.iter()
                .find(|id| ix.schemas.contains_key(id))
                .or_else(|| ids.first())
                .map(|id| (g, *id))
        };
        pick(gi).or_else(|| (0..self.idx.len()).filter(|g| *g != gi).find_map(pick))
    }

    /// The fields `node` carries on the wire in `dir`, flattened through the
    /// bases its schema's `extends` names (see the module doc), each value
    /// resolved one level below `depth`.
    pub(crate) fn model_fields(
        &self,
        gi: usize,
        node: NodeId,
        dir: WireDirection,
        depth: usize,
    ) -> ResolvedFields {
        let key = (gi, node.0, dir, depth);
        if let Some(hit) = self.memo.borrow().get(&key) {
            return hit.clone();
        }
        let mut chain = vec![node];
        let out = self.fields_in(gi, node, dir, depth, &mut chain);
        self.memo.borrow_mut().insert(key, out.clone());
        out
    }

    fn fields_in(
        &self,
        gi: usize,
        node: NodeId,
        dir: WireDirection,
        depth: usize,
        chain: &mut Vec<NodeId>,
    ) -> ResolvedFields {
        let Some(ix) = self.idx.get(gi) else {
            return ResolvedFields {
                open: true,
                ..ResolvedFields::default()
            };
        };
        let Some(schema) = ix.schemas.get(&node) else {
            return ResolvedFields {
                open: true,
                ..ResolvedFields::default()
            };
        };
        let mut out = ResolvedFields {
            fields: BTreeMap::new(),
            open: schema.open,
        };
        if schema.format == WireFormat::Go
            && ix
                .g
                .symbols
                .class_methods
                .get(&node)
                .is_some_and(|m| m.contains_key(dir.go_codec()))
        {
            out.open = true;
        }
        let module = matches!(schema.format, WireFormat::Ts | WireFormat::Dart)
            .then(|| self.module_of(gi, node))
            .flatten();
        for f in &schema.fields {
            if out.fields.contains_key(&f.name) {
                continue;
            }
            let value = self.field_value(gi, f, schema.format, module, dir, depth + 1);
            out.fields.insert(
                f.name.clone(),
                ResolvedField {
                    name: f.name.clone(),
                    member: f.field.clone().filter(|m| *m != f.name),
                    declared: Some(f.ty.clone()),
                    required: f.required,
                    bind_required: f.bind_required,
                    declared_in: Some(node),
                    value,
                },
            );
        }
        for e in &schema.extends {
            match self.extends_base(gi, node, e, schema.format) {
                Base::Model(bg, base) if chain.len() <= MAX_EXTENDS && !chain.contains(&base) => {
                    chain.push(base);
                    let sub = self.fields_in(bg, base, dir, depth, chain);
                    chain.pop();
                    out.open |= sub.open;
                    for (k, v) in sub.fields {
                        out.fields.entry(k).or_insert(v);
                    }
                }
                Base::Field(name) => {
                    let written = Shape::Named {
                        ty: e.trim().trim_start_matches('*').to_string(),
                        array: false,
                        model: None,
                    };
                    out.fields
                        .entry(name.clone())
                        .or_insert_with(|| ResolvedField {
                            name,
                            member: None,
                            declared: Some(e.clone()),
                            required: true,
                            bind_required: false,
                            declared_in: Some(node),
                            value: ResolvedShape {
                                written,
                                format: Some(schema.format),
                                model: None,
                                array: false,
                                fields: BTreeMap::new(),
                                members: Vec::new(),
                                open: true,
                            },
                        });
                }
                _ => out.open = true,
            }
        }
        out
    }

    /// The one INHERITS_FROM target of `node` whose simple name is the
    /// `extends` entry's.
    fn extends_base(&self, gi: usize, node: NodeId, entry: &str, format: WireFormat) -> Base {
        let Some(ix) = self.idx.get(gi) else {
            return Base::Unbound;
        };
        let simple = simple_base(entry);
        let mut hits = ix
            .inherits
            .get(&node)
            .into_iter()
            .flatten()
            .filter(|t| ix.name(**t) == simple);
        let (Some(&base), None) = (hits.next(), hits.next()) else {
            return Base::Unbound;
        };
        let Some(bg) = self.graph_of(base, gi) else {
            return Base::Unbound;
        };
        let target = &self.idx[bg];
        if format == WireFormat::Go
            && target.kind(base) == Some(node_kind::INTERFACE)
            && !target.schemas.contains_key(&base)
        {
            return Base::Field(simple.to_string());
        }
        Base::Model(bg, base)
    }

    /// DT.1h: every base model `node` flattens in through its schema's
    /// `extends` ([`Self::extends_base`], as [`Self::fields_in`] reads them),
    /// transitively, at most [`MAX_EXTENDS`] bases deep, cycle-safe; `node`
    /// itself is never listed. Read from the first graph holding `node`.
    fn extends_closure(&self, node: NodeId) -> BTreeSet<u64> {
        let mut out = BTreeSet::new();
        let Some(gi) = self.graphs(node).first().copied() else {
            return out;
        };
        let mut frontier = vec![(gi, node)];
        for _ in 0..MAX_EXTENDS {
            let mut next = Vec::new();
            for (g, n) in frontier {
                let Some(schema) = self.idx.get(g).and_then(|ix| ix.schemas.get(&n)) else {
                    continue;
                };
                for e in &schema.extends {
                    if let Base::Model(bg, base) = self.extends_base(g, n, e, schema.format)
                        && base != node
                        && out.insert(base.0)
                    {
                        next.push((bg, base));
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        out
    }

    /// `rule` pairs `member`'s kind with `center`'s (each read in the first
    /// graph holding it).
    fn takes(&self, rule: &IdentityRule, member: NodeId, center: NodeId) -> bool {
        self.kind_any(member)
            .is_some_and(|k| rule.from.contains(&k))
            && self.kind_any(center).is_some_and(|k| rule.to.contains(&k))
    }

    /// The project `id` belongs to ([`ProjectKey`]): its repo and the
    /// deepest PROJECT of that repo enclosing its file, read in the first
    /// graph holding it.
    fn project_of(&self, m: &MergedGraph, projects: &ProjectPaths, id: NodeId) -> ProjectKey {
        let Some(gi) = self.graphs(id).first().copied() else {
            return (0, None);
        };
        let repo = m.graphs[gi].repo;
        let owner = self.idx[gi]
            .file_of
            .get(&id)
            .and_then(|f| projects.owner_of(repo, f))
            .map(str::to_string);
        (repo.0, owner)
    }

    /// The kind of `id` in the first graph holding it.
    fn kind_any(&self, id: NodeId) -> Option<NodeKindId> {
        let gi = *self.graphs(id).first()?;
        self.idx.get(gi)?.kind(id)
    }

    /// A model field's value: its model from the builder's `model` qname,
    /// or for a TS / Dart model its element type bound in `module`.
    fn field_value(
        &self,
        gi: usize,
        f: &ModelField,
        format: WireFormat,
        module: Option<NodeId>,
        dir: WireDirection,
        depth: usize,
    ) -> ResolvedShape {
        let (elem, _) = element_type(&f.ty);
        let written = Shape::Named {
            ty: elem,
            array: f.array,
            model: f.model.clone(),
        };
        let names = match module {
            Some(m) if f.model.is_none() => Names::Client(Some((gi, m))),
            _ => Names::Written,
        };
        self.named(gi, names, written, Some(format), dir, depth)
    }

    /// A [`Shape::Named`] / [`Shape::Call`] / [`Shape::Chain`] value: its
    /// model from the written `model` qname, else (client names) the bound
    /// element type, expanded unless past [`MAX_DEPTH`].
    fn named(
        &self,
        gi: usize,
        names: Names,
        written: Shape,
        format: Option<WireFormat>,
        dir: WireDirection,
        depth: usize,
    ) -> ResolvedShape {
        let (ty, array, model_q) = match &written {
            Shape::Named { ty, array, model } => (Some(ty.as_str()), *array, model.as_deref()),
            Shape::Call { array, model, .. } | Shape::Chain { array, model, .. } => {
                (None, *array, model.as_deref())
            }
            _ => (None, false, None),
        };
        let kind = ty.map(|t| kind_of(format, t));
        let model = match (model_q, names, ty, &kind) {
            (Some(q), _, _, _) => self.model_by_qname(gi, q),
            (None, Names::Client(Some((mg, m))), Some(t), Some(JsonKind::Unknown(_))) => {
                self.resolve_client_type(mg, m, t)
            }
            _ => None,
        };
        let mut out = ResolvedShape {
            written,
            format,
            model: model.map(|(_, id)| id),
            array,
            fields: BTreeMap::new(),
            members: Vec::new(),
            open: false,
        };
        match model {
            Some((mg, id)) if depth < MAX_DEPTH => {
                let rf = self.model_fields(mg, id, dir, depth);
                out.fields = rf.fields;
                out.open = rf.open;
            }
            Some(_) => out.open = true,
            // A call or chain no builder typed, an unbound name, `any`, a map:
            // its keys are unknown.
            None => {
                out.open = match kind {
                    None => true,
                    Some(k) => matches!(k, JsonKind::Unknown(_) | JsonKind::Any | JsonKind::Object),
                }
            }
        }
        out
    }

    /// `shape` resolved in graph `gi` at `depth`.
    fn resolve(
        &self,
        gi: usize,
        names: Names,
        shape: &Shape,
        format: Option<WireFormat>,
        dir: WireDirection,
        depth: usize,
    ) -> ResolvedShape {
        let bare = |open: bool| ResolvedShape {
            written: shape.clone(),
            format,
            model: None,
            array: false,
            fields: BTreeMap::new(),
            members: Vec::new(),
            open,
        };
        match shape {
            Shape::Named { .. } | Shape::Call { .. } | Shape::Chain { .. } => {
                self.named(gi, names, shape.clone(), format, dir, depth)
            }
            Shape::Object { fields, open } => {
                let mut out = bare(*open);
                if depth >= MAX_DEPTH {
                    out.open = true;
                    return out;
                }
                for sf in fields {
                    if out.fields.contains_key(&sf.name) {
                        continue;
                    }
                    let value = self.resolve(gi, names, &sf.shape, format, dir, depth + 1);
                    out.fields.insert(
                        sf.name.clone(),
                        ResolvedField {
                            name: sf.name.clone(),
                            member: sf.field.clone().filter(|m| *m != sf.name),
                            declared: None,
                            required: sf.required,
                            bind_required: sf.bind_required,
                            declared_in: None,
                            value,
                        },
                    );
                }
                out
            }
            Shape::Union(members) => {
                let mut out = bare(false);
                out.members = members
                    .iter()
                    .map(|m| self.resolve(gi, names, m, format, dir, depth))
                    .collect();
                out.open = out.members.iter().any(|m| m.open);
                out
            }
            Shape::Open { .. } => bare(true),
        }
    }

    /// The side of `route`: its qname, the pairing's rule and its handlers
    /// (each built once, counted into `stats` once).
    fn route_side(
        &self,
        route: NodeId,
        rule: &str,
        handlers: &mut HashMap<NodeId, RouteHandler>,
        stats: &mut ClientWireStats,
    ) -> RouteSide {
        let at = self.graphs(route);
        let route_qname = at.first().map_or("", |g| self.qname(*g, route)).to_string();
        let mut targets: Vec<(usize, NodeId)> = Vec::new();
        for &g in at {
            for &h in self.idx[g].handled_by.get(&route).into_iter().flatten() {
                if !targets.iter().any(|(_, t)| *t == h) {
                    targets.push((g, h));
                }
            }
        }
        let mut out: Vec<RouteHandler> = targets
            .into_iter()
            .map(|(g, h)| {
                handlers
                    .entry(h)
                    .or_insert_with(|| self.route_handler(g, h, stats))
                    .clone()
            })
            .collect();
        out.sort_by(|a, b| a.qname.cmp(&b.qname));
        RouteSide {
            route,
            route_qname,
            rule: rule.to_string(),
            handlers: out,
        }
    }

    /// `handler`'s server sites per direction (every graph holding it,
    /// identical payloads once), resolved in the graph its HANDLED_BY edge
    /// sits in.
    fn route_handler(
        &self,
        gi: usize,
        handler: NodeId,
        stats: &mut ClientWireStats,
    ) -> RouteHandler {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut sites: Vec<ServerWire> = Vec::new();
        for &g in self.graphs(handler) {
            let ix = &self.idx[g];
            let Some(n) = ix.at.get(&handler).and_then(|i| ix.g.nodes.get(*i)) else {
                continue;
            };
            for c in n.cells.iter().filter(|c| c.kind == cell_type::MESSAGE_TYPE) {
                if let CellPayload::Json(p) | CellPayload::Text(p) = &c.payload
                    && seen.insert(p.clone())
                    && let Some(w) = ServerWire::from_cell(c)
                {
                    sites.push(w);
                }
            }
        }
        let format = self.idx[gi]
            .file_of
            .get(&handler)
            .and_then(|f| format_for_file(f));
        let mut response = Vec::new();
        let mut request = Vec::new();
        for w in sites {
            let Some(dir) = WireDirection::ALL
                .into_iter()
                .find(|d| d.server_form() == w.form)
            else {
                continue;
            };
            if dir == WireDirection::Response && w.status.is_none() {
                stats.server_unknown_status += 1;
                continue;
            }
            if dir == WireDirection::Response && !w.known_2xx() {
                continue;
            }
            let shape = self.resolve(gi, Names::Written, &w.shape, format, dir, 0);
            stats.server_sites += 1;
            stats.open += usize::from(shape.open);
            let site = ServerSite {
                handler,
                form: w.form,
                line: w.line,
                status: w.status,
                shape,
            };
            match dir {
                WireDirection::Response => response.push(site),
                WireDirection::Request => request.push(site),
            }
        }
        let by_line = |a: &ServerSite, b: &ServerSite| (a.line, a.status).cmp(&(b.line, b.status));
        response.sort_by(by_line);
        request.sort_by(by_line);
        RouteHandler {
            handler,
            qname: self.qname(gi, handler).to_string(),
            response,
            request,
        }
    }
}

// ---- wire pairs (DT.1m) -----------------------------------------------------

/// Response: a name the client requires and the server never writes.
const FIELD_MISSING: &str = "json_field_missing";
/// Response: an optional client name the server never writes.
const FIELD_OPTIONAL_ABSENT: &str = "json_field_optional_absent";
/// Response: a name the server writes and the client never reads.
const FIELD_UNREAD: &str = "json_field_unread";
/// Response: the client requires a name the server may omit (Go omitempty).
const FIELD_MAY_BE_ABSENT: &str = "json_field_may_be_absent";
/// Either direction: the two sides' JSON kinds differ, both known.
const TYPE_MISMATCH: &str = "json_type_mismatch";
/// Request: a name the client sends and the server never binds.
const FIELD_DROPPED: &str = "json_field_dropped";
/// Request: a name the server's binder requires and the client never sends.
const REQUIRED_MISSING: &str = "json_required_missing";
/// Request: a name the server binds and the client never sends.
const FIELD_UNSET: &str = "json_field_unset";
/// Request: a name the server's binder requires and the client may leave
/// out (`discount?: string` against Go `binding:"required"`): a body
/// without it is a 400 (DT.1n, the DT.1m request gap).
const REQUIRED_MAY_BE_UNSET: &str = "json_required_may_be_unset";

/// The handler has several distinct 2xx shapes and none overlaps the client
/// most (a response), or several distinct binds (a request): nothing is
/// compared.
const NOTE_AMBIGUOUS_SERVER: &str = "ambiguous_server";
/// A compared side is open: no name is claimed absent from it.
const NOTE_OPEN: &str = "open";
/// A side is a union, compared as its members' merged fields.
const NOTE_UNION: &str = "union";

/// One difference between a client and a server side, at a flattened JSON
/// path.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WireChange {
    /// The flattened JSON path (`customerName`, `items[].sku`, `[].id` under
    /// a root array): the client's names, the server's for a server-only
    /// name.
    pub path: String,
    /// `json_field_missing`, `json_field_optional_absent`,
    /// `json_field_unread`, `json_field_may_be_absent`,
    /// `json_type_mismatch` (a response); `json_field_dropped`,
    /// `json_required_missing`, `json_required_may_be_unset`,
    /// `json_field_unset`, `json_type_mismatch` (a request).
    pub rule: &'static str,
    /// `json_field_missing`, `json_required_missing`,
    /// `json_required_may_be_unset` and `json_type_mismatch` break the
    /// exchange.
    pub breaking: bool,
    /// The client's type text at `path` (declared, else as written); `None`
    /// when the client does not list the name.
    pub client: Option<String>,
    /// The server's type text at `path`; `None` when the server does not
    /// list the name.
    pub server: Option<String>,
}

/// One client call site against one handler of one route it is paired with,
/// in one direction: the server side picked and the changes between them.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WirePair {
    pub direction: WireDirection,
    pub endpoint: NodeId,
    pub endpoint_qname: String,
    /// The endpoint's LB.8 owner segment, if any.
    pub owner: Option<String>,
    /// The call site's repo-relative file.
    pub file: String,
    /// The call site's 1-based line.
    pub line: u32,
    pub route: NodeId,
    pub route_qname: String,
    /// The HTTP tier of the pairing (`exact`, `base_fold`, ..).
    pub rule: String,
    pub handler: NodeId,
    pub handler_qname: String,
    /// The picked server site's 1-based line; `None` when ambiguous.
    pub server_line: Option<u32>,
    /// The client's type grammar.
    pub client_format: Option<WireFormat>,
    /// The server site's type grammar.
    pub server_format: Option<WireFormat>,
    /// The client root's model (an array's element model).
    pub client_model: Option<NodeId>,
    /// The picked server root's model; `None` for an inline envelope and
    /// when ambiguous.
    pub server_model: Option<NodeId>,
    /// Sorted by (path, rule).
    pub changes: Vec<WireChange>,
    /// `ambiguous_server`, `open`, `union`, sorted.
    pub notes: Vec<&'static str>,
}

/// One exchange position where a client model met a server model and
/// `confirms` held (at least one shared JSON name, at least half of the
/// smaller side's).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelJoin {
    pub direction: WireDirection,
    pub route_qname: String,
    /// The joining call site's file.
    pub file: String,
    /// The joining call site's 1-based line.
    pub line: u32,
    pub handler_qname: String,
    /// The models' position in the exchange: empty at the root, else the
    /// flattened path holding them (`items[]`, `invoices[]`, `[]` for a root
    /// array).
    pub path: String,
    /// JSON names matched on both sides at this position.
    pub overlap: usize,
    pub client_fields: usize,
    pub server_fields: usize,
    /// A change at this model's level (not inside a nested model level).
    pub diverged: bool,
}

/// A client model and a server model the exchanges confirm are one model.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelPair {
    pub client: NodeId,
    pub client_qname: String,
    pub server: NodeId,
    pub server_qname: String,
    /// The largest overlap of any join.
    pub overlap: usize,
    /// Any join diverged.
    pub diverged: bool,
    /// Every position joining the two, sorted by (route qname, direction,
    /// hit file, line, handler qname, path).
    pub joins: Vec<ModelJoin>,
}

/// The counts behind [`WirePairs::marker`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WirePairStats {
    pub pairs: usize,
    /// Of `pairs`: response pairs.
    pub response: usize,
    /// Of `pairs`: request pairs.
    pub request: usize,
    /// Distinct (client model, server model) pairs.
    pub model_pairs: usize,
    /// Changes over every pair.
    pub changes: usize,
    /// Of `changes`: breaking.
    pub breaking: usize,
    /// Pairs noted `ambiguous_server`.
    pub ambiguous_server: usize,
    /// Pairs noted `open`.
    pub open: usize,
}

/// Every [`WirePair`] and [`ModelPair`] of a merged graph, sorted, with its
/// counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WirePairs {
    /// Sorted by (route qname, direction, hit file, line, handler qname).
    pub pairs: Vec<WirePair>,
    /// Sorted by (client qname, server qname, first join).
    pub model_pairs: Vec<ModelPair>,
    pub stats: WirePairStats,
}

impl WirePairs {
    /// The fired_on line.
    pub fn marker(&self) -> String {
        let s = &self.stats;
        format!(
            "[models] pairs={} response={} request={} model_pairs={} changes={} breaking={} \
             ambiguous_server={} open={}",
            s.pairs,
            s.response,
            s.request,
            s.model_pairs,
            s.changes,
            s.breaking,
            s.ambiguous_server,
            s.open,
        )
    }
}

/// Every [`client_wires`] call site compared with the server side of each
/// handler it reaches, per direction (see the module doc's DT.1m part).
pub fn wire_pairs(merged: &MergedGraph) -> WirePairs {
    pairs_of(merged, &client_wires(merged))
}

/// [`wire_pairs`] over wires already collected.
fn pairs_of(merged: &MergedGraph, cw: &ClientWires) -> WirePairs {
    let mut pairs: Vec<WirePair> = Vec::new();
    let mut found: Vec<(FoundModel, ModelJoin)> = Vec::new();
    for w in &cw.wires {
        for side in &w.routes {
            for h in &side.handlers {
                for dir in WireDirection::ALL {
                    let Some(client) = w.client(dir) else {
                        continue;
                    };
                    let sites = h.sites(dir);
                    let Some(first) = sites.first() else {
                        continue;
                    };
                    let mut pair = WirePair {
                        direction: dir,
                        endpoint: w.endpoint,
                        endpoint_qname: w.endpoint_qname.clone(),
                        owner: w.owner.clone(),
                        file: w.file.clone(),
                        line: w.line,
                        route: side.route,
                        route_qname: side.route_qname.clone(),
                        rule: side.rule.clone(),
                        handler: h.handler,
                        handler_qname: h.qname.clone(),
                        server_line: None,
                        client_format: w.format,
                        server_format: first.shape.format,
                        client_model: client.model,
                        server_model: None,
                        changes: Vec::new(),
                        notes: Vec::new(),
                    };
                    let Some(site) = pick_server(dir, client, sites) else {
                        pair.notes.push(NOTE_AMBIGUOUS_SERVER);
                        pairs.push(pair);
                        continue;
                    };
                    let cmp = compare(dir, client, &site.shape, "");
                    pair.server_line = Some(site.line);
                    pair.server_format = site.shape.format;
                    pair.server_model = site.shape.model;
                    pair.changes = cmp.changes;
                    pair.notes = cmp.notes.into_iter().collect();
                    for m in cmp.models {
                        let join = ModelJoin {
                            direction: dir,
                            route_qname: pair.route_qname.clone(),
                            file: pair.file.clone(),
                            line: pair.line,
                            handler_qname: pair.handler_qname.clone(),
                            path: m.path.clone(),
                            overlap: m.overlap,
                            client_fields: m.client_fields,
                            server_fields: m.server_fields,
                            diverged: m.diverged,
                        };
                        found.push((m, join));
                    }
                    pairs.push(pair);
                }
            }
        }
    }
    pairs.sort_by_cached_key(|p| {
        (
            p.route_qname.clone(),
            p.direction,
            p.file.clone(),
            p.line,
            p.handler_qname.clone(),
            p.endpoint_qname.clone(),
        )
    });

    let mut index: HashMap<(NodeId, NodeId), usize> = HashMap::new();
    let mut model_pairs: Vec<ModelPair> = Vec::new();
    for (m, join) in found {
        let at = *index.entry((m.client, m.server)).or_insert_with(|| {
            model_pairs.push(ModelPair {
                client: m.client,
                client_qname: qname_of(merged, m.client),
                server: m.server,
                server_qname: qname_of(merged, m.server),
                overlap: 0,
                diverged: false,
                joins: Vec::new(),
            });
            model_pairs.len() - 1
        });
        if let Some(mp) = model_pairs.get_mut(at) {
            mp.overlap = mp.overlap.max(join.overlap);
            mp.diverged |= join.diverged;
            mp.joins.push(join);
        }
    }
    let join_key = |j: &ModelJoin| {
        (
            j.route_qname.clone(),
            j.direction,
            j.file.clone(),
            j.line,
            j.handler_qname.clone(),
            j.path.clone(),
        )
    };
    for mp in &mut model_pairs {
        mp.joins.sort_by_cached_key(join_key);
    }
    model_pairs.sort_by_cached_key(|mp| {
        (
            mp.client_qname.clone(),
            mp.server_qname.clone(),
            mp.joins.first().map(join_key),
        )
    });

    let count = |dir: WireDirection| pairs.iter().filter(|p| p.direction == dir).count();
    let noted = |note: &str| pairs.iter().filter(|p| p.notes.contains(&note)).count();
    let stats = WirePairStats {
        pairs: pairs.len(),
        response: count(WireDirection::Response),
        request: count(WireDirection::Request),
        model_pairs: model_pairs.len(),
        changes: pairs.iter().map(|p| p.changes.len()).sum(),
        breaking: pairs
            .iter()
            .flat_map(|p| &p.changes)
            .filter(|c| c.breaking)
            .count(),
        ambiguous_server: noted(NOTE_AMBIGUOUS_SERVER),
        open: noted(NOTE_OPEN),
    };
    WirePairs {
        pairs,
        model_pairs,
        stats,
    }
}

/// The qname of `id` in the first graph that names it.
fn qname_of(m: &MergedGraph, id: NodeId) -> String {
    m.graphs
        .iter()
        .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
        .unwrap_or_default()
}

/// The server site a client side is compared with. A response reads the
/// handler's distinct known-2xx shapes ([`same_wire`]): one is it; of
/// several, the one whose top-level names overlap the client's most, a tie
/// none. A request needs exactly one distinct bind.
fn pick_server<'s>(
    dir: WireDirection,
    client: &ResolvedShape,
    sites: &'s [ServerSite],
) -> Option<&'s ServerSite> {
    let mut distinct: Vec<&ServerSite> = Vec::new();
    for s in sites {
        if !distinct.iter().any(|d| same_wire(&d.shape, &s.shape)) {
            distinct.push(s);
        }
    }
    match distinct.as_slice() {
        [one] => return Some(one),
        [] => return None,
        _ if dir == WireDirection::Request => return None,
        _ => {}
    }
    let names = |s: &ResolvedShape| -> Vec<String> {
        object_view(s).map_or_else(Vec::new, |v| v.fields.keys().cloned().collect())
    };
    let theirs = names(client);
    let scores: Vec<usize> = distinct
        .iter()
        .map(|d| {
            let ours = names(&d.shape);
            match_names(
                theirs.iter().map(String::as_str),
                ours.iter().map(String::as_str),
                false,
            )
            .len()
        })
        .collect();
    let best = scores.iter().copied().max()?;
    let mut winners = distinct
        .iter()
        .zip(&scores)
        .filter(|(_, s)| **s == best)
        .map(|(d, _)| *d);
    match (winners.next(), winners.next()) {
        (Some(one), None) => Some(one),
        _ => None,
    }
}

/// Two server values travel alike: one model (with the same array-ness and
/// openness), or inline values whose names, requiredness and values match,
/// or scalars of one JSON kind (as written when no kind is known).
fn same_wire(a: &ResolvedShape, b: &ResolvedShape) -> bool {
    if a.model != b.model
        || a.array != b.array
        || a.open != b.open
        || a.fields.len() != b.fields.len()
        || a.members.len() != b.members.len()
    {
        return false;
    }
    if a.model.is_some() {
        return true;
    }
    let fields = a.fields.iter().zip(&b.fields).all(|((ka, fa), (kb, fb))| {
        ka == kb
            && fa.required == fb.required
            && fa.bind_required == fb.bind_required
            && same_wire(&fa.value, &fb.value)
    });
    let members = a
        .members
        .iter()
        .zip(&b.members)
        .all(|(x, y)| same_wire(x, y));
    let kinds = match (element_kind(a), element_kind(b)) {
        (Some(x), Some(y)) => x == y,
        (None, None) => a.written == b.written,
        _ => false,
    };
    fields && members && kinds
}

/// The names of `client` and `server` that match: exact first, then, when
/// `fold`, an ASCII case-insensitive match among the server names still
/// free (encoding/json's decode folding; the least such name), each server
/// name used once. Sorted by client name.
fn match_names<'a>(
    client: impl IntoIterator<Item = &'a str>,
    server: impl IntoIterator<Item = &'a str>,
    fold: bool,
) -> Vec<(&'a str, &'a str)> {
    let client: BTreeSet<&str> = client.into_iter().collect();
    let server: BTreeSet<&str> = server.into_iter().collect();
    let mut used: BTreeSet<&str> = BTreeSet::new();
    let mut out: Vec<(&str, &str)> = Vec::new();
    let mut rest: Vec<&str> = Vec::new();
    for c in &client {
        if server.contains(c) {
            used.insert(c);
            out.push((c, c));
        } else {
            rest.push(c);
        }
    }
    if fold {
        for c in rest {
            if let Some(s) = server
                .iter()
                .find(|s| !used.contains(*s) && s.eq_ignore_ascii_case(c))
            {
                used.insert(s);
                out.push((c, s));
            }
        }
    }
    out.sort_unstable();
    out
}

/// The client and server names name one model: they overlap in at least
/// one name, and in at least half of the smaller side's names. `fold` as in
/// [`match_names`]. Returns the overlap. DT.1n / DT.1k confirm their pairs
/// through it.
pub(crate) fn confirms<'a>(
    client: impl IntoIterator<Item = &'a str>,
    server: impl IntoIterator<Item = &'a str>,
    fold: bool,
) -> Option<usize> {
    let client: BTreeSet<&str> = client.into_iter().collect();
    let server: BTreeSet<&str> = server.into_iter().collect();
    let overlap = match_names(client.iter().copied(), server.iter().copied(), fold).len();
    (overlap >= 1 && 2 * overlap >= client.len().min(server.len())).then_some(overlap)
}

/// A kind that can be judged: never any, null or an unknown name.
fn judged(k: JsonKind) -> Option<JsonKind> {
    (!matches!(k, JsonKind::Any | JsonKind::Null | JsonKind::Unknown(_))).then_some(k)
}

/// The JSON kind a value is judged by at its position: an array when one
/// layer was peeled; else its declared text's kind (an unknown name is an
/// object only when it resolved to a model; a TS tuple is an array whose
/// elements are not judged); else its element's kind. `None` is never
/// judged.
fn outer_kind(declared: Option<&str>, v: &ResolvedShape) -> Option<JsonKind> {
    if v.array {
        return Some(JsonKind::Array);
    }
    match declared {
        Some(d) => match kind_of(v.format, d) {
            JsonKind::Unknown(_) => v.model.is_some().then_some(JsonKind::Object),
            k => judged(k),
        },
        None => element_kind(v),
    }
}

/// The JSON kind of `v`'s element (`v` itself when it is no array): a
/// resolved model or an inline object is an object, a written name its
/// format's kind, a union the kind its members share; a call or chain no
/// builder typed, an open value and an unbound name are never judged.
fn element_kind(v: &ResolvedShape) -> Option<JsonKind> {
    if v.model.is_some() {
        return Some(JsonKind::Object);
    }
    match &v.written {
        Shape::Named { ty, .. } => judged(kind_of(v.format, ty)),
        Shape::Object { .. } => Some(JsonKind::Object),
        Shape::Union(_) => {
            let mut kinds = v.members.iter().map(|m| outer_kind(None, m));
            let first = kinds.next()??;
            kinds.all(|k| k.as_ref() == Some(&first)).then_some(first)
        }
        Shape::Call { .. } | Shape::Chain { .. } | Shape::Open { .. } => None,
    }
}

/// `v` as an object compared name by name: a model or an inline object
/// itself; a union whose members all are (none an array) as their merged
/// fields, a name required only when every member requires it, open when
/// any member is.
fn object_view(v: &ResolvedShape) -> Option<Cow<'_, ResolvedShape>> {
    if v.model.is_some() || matches!(v.written, Shape::Object { .. }) {
        return Some(Cow::Borrowed(v));
    }
    if !matches!(v.written, Shape::Union(_)) || v.members.is_empty() {
        return None;
    }
    let mut views = Vec::with_capacity(v.members.len());
    for m in &v.members {
        if m.array {
            return None;
        }
        views.push(object_view(m)?);
    }
    let mut fields: BTreeMap<String, ResolvedField> = BTreeMap::new();
    for view in &views {
        for (name, f) in &view.fields {
            fields.entry(name.clone()).or_insert_with(|| f.clone());
        }
    }
    for (name, f) in &mut fields {
        let every = |pick: fn(&ResolvedField) -> bool| {
            views
                .iter()
                .all(|view| view.fields.get(name).is_some_and(pick))
        };
        f.required = every(|g| g.required);
        f.bind_required = every(|g| g.bind_required);
    }
    let merged = ResolvedShape {
        written: v.written.clone(),
        format: v.format,
        model: None,
        array: false,
        fields,
        members: Vec::new(),
        open: views.iter().any(|view| view.open),
    };
    Some(Cow::Owned(merged))
}

/// DT.1l's envelope-or-array rule: the member of a client union that the
/// server value's kind matches, when the union has members of more than one
/// kind (`{ activities: C[] } | C[]`, a client that accepts both shapes) and
/// the server value is no union of its own: the one member of an object
/// server value's kind (several object members: their union, compared as
/// its merged fields) or the one array member of an array server value. A
/// union matches when one member does. `None` for an all-object union
/// ([`object_view`] merges it), a member of no known kind (`any`, an unbound
/// name: it could be what travels), no member of the server's kind, two
/// array members, or a server value of no known kind.
fn union_member<'c>(
    c: &'c ResolvedShape,
    sd: Option<&str>,
    s: &ResolvedShape,
) -> Option<Cow<'c, ResolvedShape>> {
    if !matches!(c.written, Shape::Union(_))
        || matches!(s.written, Shape::Union(_))
        || object_view(c).is_some()
    {
        return None;
    }
    let ks = outer_kind(sd, s)?;
    if !matches!(ks, JsonKind::Object | JsonKind::Array) {
        return None;
    }
    // Every member's kind is known: a member of no known kind (`any`, a
    // name that bound nothing) could be what travels.
    let kinds: Vec<JsonKind> = c
        .members
        .iter()
        .map(|m| outer_kind(None, m))
        .collect::<Option<_>>()?;
    let same: Vec<&ResolvedShape> = c
        .members
        .iter()
        .zip(&kinds)
        .filter(|(_, k)| **k == ks)
        .map(|(m, _)| m)
        .collect();
    match (same.as_slice(), &ks) {
        ([one], _) => Some(Cow::Borrowed(*one)),
        ([_, _, ..], JsonKind::Object) => Some(Cow::Owned(ResolvedShape {
            written: Shape::Union(same.iter().map(|m| m.written.clone()).collect()),
            format: c.format,
            model: None,
            array: false,
            fields: BTreeMap::new(),
            open: same.iter().any(|m| m.open),
            members: same.into_iter().cloned().collect(),
        })),
        _ => None,
    }
}

/// A value's type text: its declared text, else as written (an array in its
/// format's spelling).
fn type_text(declared: Option<&str>, v: &ResolvedShape) -> String {
    if let Some(d) = declared {
        return d.trim().to_string();
    }
    match &v.written {
        Shape::Named {
            ty, array: true, ..
        } => match v.format {
            Some(WireFormat::Go) => format!("[]{ty}"),
            Some(WireFormat::Dart) => format!("List<{ty}>"),
            Some(WireFormat::Graphql) => format!("[{ty}]"),
            Some(WireFormat::Ts) | None => format!("{ty}[]"),
        },
        Shape::Named { ty, .. } => ty.clone(),
        Shape::Call { chain, .. } | Shape::Chain { chain, .. } => chain.clone(),
        Shape::Object { .. } => "object".to_string(),
        Shape::Union(_) => v
            .members
            .iter()
            .map(|m| type_text(None, m))
            .collect::<Vec<_>>()
            .join(" | "),
        Shape::Open { raw } => raw.clone(),
    }
}

/// `path` extended by one JSON name.
fn join_path(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

/// A confirmed model pair met inside one comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FoundModel {
    client: NodeId,
    server: NodeId,
    path: String,
    overlap: usize,
    client_fields: usize,
    server_fields: usize,
    diverged: bool,
}

/// What [`compare`] found.
#[derive(Debug)]
struct Compared {
    dir: WireDirection,
    changes: Vec<WireChange>,
    models: Vec<FoundModel>,
    notes: BTreeSet<&'static str>,
}

/// The changes and the confirmed model pairs between a client side and a
/// server side in `dir`, from `path` (empty: the root). Changes are sorted
/// by (path, rule), model pairs by path.
fn compare(
    dir: WireDirection,
    client: &ResolvedShape,
    server: &ResolvedShape,
    path: &str,
) -> Compared {
    let mut c = Compared {
        dir,
        changes: Vec::new(),
        models: Vec::new(),
        notes: BTreeSet::new(),
    };
    c.values(None, client, None, server, path, 0);
    c.changes
        .sort_by(|a, b| (&a.path, a.rule).cmp(&(&b.path, b.rule)));
    c.models.sort_by(|a, b| a.path.cmp(&b.path));
    c
}

impl Compared {
    fn push(
        &mut self,
        path: &str,
        rule: &'static str,
        breaking: bool,
        client: Option<String>,
        server: Option<String>,
    ) {
        self.changes.push(WireChange {
            path: path.to_string(),
            rule,
            breaking,
            client,
            server,
        });
    }

    /// Two values at one position (a model field's with its declared text):
    /// their kinds, then their array elements, then their names. Returns
    /// how many changes it pushed that the enclosing level owns (a nested
    /// model level owns its own).
    fn values(
        &mut self,
        cd: Option<&str>,
        c: &ResolvedShape,
        sd: Option<&str>,
        s: &ResolvedShape,
        path: &str,
        depth: usize,
    ) -> usize {
        if matches!(c.written, Shape::Union(_)) || matches!(s.written, Shape::Union(_)) {
            self.notes.insert(NOTE_UNION);
        }
        if let Some(member) = union_member(c, sd, s) {
            return self.values(None, &member, sd, s, path, depth);
        }
        let (kc, ks) = (outer_kind(cd, c), outer_kind(sd, s));
        if let (Some(a), Some(b)) = (&kc, &ks)
            && a != b
        {
            let (ct, st) = (type_text(cd, c), type_text(sd, s));
            self.push(path, TYPE_MISMATCH, true, Some(ct), Some(st));
            return 1;
        }
        match (kc == Some(JsonKind::Array), ks == Some(JsonKind::Array)) {
            (true, true) => {
                // A tuple's elements are no one type: not judged.
                if !(c.array && s.array) {
                    return 0;
                }
                let elem = format!("{path}[]");
                if let (Some(a), Some(b)) = (element_kind(c), element_kind(s))
                    && a != b
                {
                    let (ct, st) = (type_text(cd, c), type_text(sd, s));
                    self.push(&elem, TYPE_MISMATCH, true, Some(ct), Some(st));
                    return 1;
                }
                self.objects(c, s, &elem, depth)
            }
            (false, false) => self.objects(c, s, path, depth),
            // An array against a value of no known kind: nothing is judged.
            _ => {
                if c.open || s.open {
                    self.notes.insert(NOTE_OPEN);
                }
                0
            }
        }
    }

    /// Two elements compared name by name when both are objects; an object
    /// against an open value claims nothing (noted).
    fn objects(&mut self, c: &ResolvedShape, s: &ResolvedShape, path: &str, depth: usize) -> usize {
        match (object_view(c), object_view(s)) {
            (Some(co), Some(so)) => self.level(&co, &so, path, depth),
            (Some(_), None) if s.open => {
                self.notes.insert(NOTE_OPEN);
                0
            }
            (None, Some(_)) if c.open => {
                self.notes.insert(NOTE_OPEN);
                0
            }
            _ => 0,
        }
    }

    /// One object level: the names on one side only (none is claimed absent
    /// from an open side), then each matched name's requiredness and value.
    /// When both sides are models it is a model level: it owns its changes
    /// and, when [`confirms`] holds, is a model pair.
    fn level(&mut self, co: &ResolvedShape, so: &ResolvedShape, path: &str, depth: usize) -> usize {
        if depth > MAX_DEPTH {
            return 0;
        }
        if co.open || so.open {
            self.notes.insert(NOTE_OPEN);
        }
        let fold = self.dir == WireDirection::Request && so.format == Some(WireFormat::Go);
        let cnames = co.fields.keys().map(String::as_str);
        let snames = so.fields.keys().map(String::as_str);
        let matched = match_names(cnames.clone(), snames.clone(), fold);
        let on_client: BTreeSet<&str> = matched.iter().map(|(c, _)| *c).collect();
        let on_server: BTreeSet<&str> = matched.iter().map(|(_, s)| *s).collect();
        let mut own = 0;
        if !so.open {
            for (name, f) in &co.fields {
                if on_client.contains(name.as_str()) {
                    continue;
                }
                let (rule, breaking) = match (self.dir, f.required) {
                    (WireDirection::Response, true) => (FIELD_MISSING, true),
                    (WireDirection::Response, false) => (FIELD_OPTIONAL_ABSENT, false),
                    (WireDirection::Request, _) => (FIELD_DROPPED, false),
                };
                let text = type_text(f.declared.as_deref(), &f.value);
                self.push(&join_path(path, name), rule, breaking, Some(text), None);
                own += 1;
            }
        }
        if !co.open {
            for (name, f) in &so.fields {
                if on_server.contains(name.as_str()) {
                    continue;
                }
                let (rule, breaking) = match (self.dir, f.bind_required) {
                    (WireDirection::Response, _) => (FIELD_UNREAD, false),
                    (WireDirection::Request, true) => (REQUIRED_MISSING, true),
                    (WireDirection::Request, false) => (FIELD_UNSET, false),
                };
                let text = type_text(f.declared.as_deref(), &f.value);
                self.push(&join_path(path, name), rule, breaking, None, Some(text));
                own += 1;
            }
        }
        for (cn, sn) in &matched {
            let (Some(cf), Some(sf)) = (co.fields.get(*cn), so.fields.get(*sn)) else {
                continue;
            };
            let at = join_path(path, cn);
            if self.dir == WireDirection::Response && cf.required && !sf.required {
                let ct = type_text(cf.declared.as_deref(), &cf.value);
                let st = type_text(sf.declared.as_deref(), &sf.value);
                self.push(&at, FIELD_MAY_BE_ABSENT, false, Some(ct), Some(st));
                own += 1;
            }
            if self.dir == WireDirection::Request && !cf.required && sf.bind_required {
                let ct = type_text(cf.declared.as_deref(), &cf.value);
                let st = type_text(sf.declared.as_deref(), &sf.value);
                self.push(&at, REQUIRED_MAY_BE_UNSET, true, Some(ct), Some(st));
                own += 1;
            }
            own += self.values(
                cf.declared.as_deref(),
                &cf.value,
                sf.declared.as_deref(),
                &sf.value,
                &at,
                depth + 1,
            );
        }
        let (Some(client), Some(server)) = (co.model, so.model) else {
            return own;
        };
        if let Some(overlap) = confirms(cnames, snames, fold) {
            self.models.push(FoundModel {
                client,
                server,
                path: path.to_string(),
                overlap,
                client_fields: co.fields.len(),
                server_fields: so.fields.len(),
                diverged: own > 0,
            });
        }
        0
    }
}

// ---- DT.1h: http_model identity ---------------------------------------------

/// The pass name: the EVIDENCE emitter is `resolver:models`, the stamp of the
/// engine's `models` Resolve pass.
const PASS: &str = "models";

/// The `models` Resolve pass (DT.1h): every confirmed client <-> server model
/// pair [`wire_pairs`] finds becomes an `http_model` SAME_AS class, one star
/// per server model (see [`model_stars`] for which pairs make a class).
///
/// **Rule.** Which kinds a class pairs and how surely are the code domain's
/// `http_model` row of `CODE_TABLES.identity_rules`, read by name: a client
/// INTERFACE / CLASS and a server STRUCT, DERIVED (Medium), walked. The
/// identity resolver never writes it (its rules are `same_as::RULES`), and
/// this pass writes only `same_as::MODEL_RULES`.
///
/// **Star.** The center is the server model: the one side every client of
/// the exchange agrees on, chosen by content, never by a NodeId. Every client
/// model of the class gets one `client -> server` SAME_AS edge with ONE
/// EVIDENCE cell: emitter `resolver:models`, rule `http_model`, or
/// `http_model/diverged` when any exchange joining the two showed a change at
/// that model's level ([`ModelPair::diverged`]), located at the first joining
/// call site (its file, line - 1: EVIDENCE lines are 0-based). No node or
/// node cell is written.
///
/// **Merge-safe and deterministic.** Its inputs are [`wire_pairs`]' (every one
/// persisted, HTTP_CALLS re-run by a merge before this pass), every list is
/// sorted by content, and maps are only probed.
///
/// **DT.1k.** The pass also writes the `graphql_model` classes
/// ([`graphql_model_identity_in`]): a GraphQL client call's result model and
/// gqlgen's generated model joined with the schema type the call's root
/// field returns, the type's MESSAGE_TYPE the center.
///
/// fired_on, in this order: once per resolve where an ENDPOINT hit has an
/// HTTP_CALLS pair, [`ClientWires::marker`] and [`WirePairs::marker`]; once
/// per resolve where a callable carries a `graphql_result` site, `[models]
/// graphql results=<n> documents_resolved=<d> generated_models=<m>
/// classes=<c>`; LC.3c's `[evidence-rules] resolver=models http_model=<h>
/// graphql_model=<g>` (only with an edge); then, with the HTTP pair,
/// `[models] classes=<c> edges=<e> diverged=<d> ambiguous_client=<a>
/// ambiguous_project=<b> embed_reduced=<r>` (the `http_model` classes; `d`
/// the classes with a diverged edge; `a`, `b` and `r` count client models,
/// see [`model_stars`]).
/// grep: `... 2>&1 | grep '^\[models\] \(classes\|graphql results\)='`
pub struct ModelResolver;

impl CrossGraphResolver for ModelResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let ctx = OnceCell::new();
        let (mut edges, http) = model_identity_in(merged, &ctx);
        let (graphql, gql) = graphql_model_identity_in(merged, &ctx);
        let mut tally = RuleTally::new(PASS, &same_as::MODEL_RULES);
        tally.add(same_as::RULE_HTTP_MODEL, edges.len());
        tally.add(same_as::RULE_GRAPHQL_MODEL, graphql.len());
        if let Some(r) = &http {
            eprintln!("{}", r.client_wires);
            eprintln!("{}", r.wire_pairs);
        }
        if let Some(g) = &gql {
            eprintln!("{}", g.line());
        }
        tally.report();
        if let Some(r) = &http {
            eprintln!("{}", r.line());
        }
        edges.extend(graphql);
        drop(ctx);
        merged.cross_edges.extend(edges);
    }
}

/// The counts behind the `[models] classes=` line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct StarStats {
    /// Client models whose candidate servers held two after the embed rule.
    ambiguous_client: usize,
    /// Client models dropped because another client model of their project
    /// pairs the same server model.
    ambiguous_project: usize,
    /// Client models whose candidate servers the embed rule reduced.
    embed_reduced: usize,
}

/// One server model's class: the server and the pair of each client model
/// joining it, clients sorted by (qname, first join).
struct ModelStar<'p> {
    server: NodeId,
    clients: Vec<&'p ModelPair>,
}

/// The project a client model belongs to: its repo and the path of the
/// deepest PROJECT of that repo enclosing its file (`None` with no such
/// PROJECT, or no file: then the repo is the unit).
type ProjectKey = (u64, Option<String>);

/// The `http_model` classes of `pairs` (sorted as [`WirePairs::model_pairs`]),
/// keeping a pair only when `takes` holds (its kinds are the rule's):
///
/// 1. For each client model c, P(c) is its confirmed server models.
/// 2. A server model in P(c) that embeds another member of P(c) (`embeds(s,
///    other)`: one of the bases its schema's `extends` flattens in,
///    transitively) is dropped: the client reads the shared base, and the
///    embedder is that base plus an envelope (`Listing` against `Listing`
///    and `ListingWithSeller`). A client so reduced counts `embed_reduced`.
/// 3. |P(c)| == 1 pairs c with that server; two or more is `ambiguous_client`
///    (`Summary` against `DailySummary` and `WeeklySummary`): no class.
/// 4. Per server model, two or more client models of one project
///    (`project(c)`) are all dropped (`ambiguous_project`): one project's
///    client keeps one model per server shape, so two are a request and a
///    response view, never one model. Client models of different projects
///    (two web apps of one API) join one class.
///
/// One star per server model with a client left, by (server qname, server
/// id): the id breaks a tie only for totality.
fn model_stars<'p>(
    pairs: &'p [ModelPair],
    takes: impl Fn(&ModelPair) -> bool,
    embeds: impl Fn(NodeId, NodeId) -> bool,
    project: impl Fn(NodeId) -> ProjectKey,
) -> (Vec<ModelStar<'p>>, StarStats) {
    let mut stats = StarStats::default();
    let mut order: Vec<(NodeId, Vec<&ModelPair>)> = Vec::new();
    let mut at: HashMap<NodeId, usize> = HashMap::new();
    for p in pairs.iter().filter(|p| takes(p)) {
        let i = *at.entry(p.client).or_insert_with(|| {
            order.push((p.client, Vec::new()));
            order.len() - 1
        });
        order[i].1.push(p);
    }

    let mut by_server: BTreeMap<(&str, u64), Vec<&ModelPair>> = BTreeMap::new();
    for (_, servers) in &order {
        let kept: Vec<&ModelPair> = servers
            .iter()
            .copied()
            .filter(|s| {
                !servers
                    .iter()
                    .any(|o| o.server != s.server && embeds(s.server, o.server))
            })
            .collect();
        if kept.len() < servers.len() {
            stats.embed_reduced += 1;
        }
        match kept.as_slice() {
            [one] => by_server
                .entry((one.server_qname.as_str(), one.server.0))
                .or_default()
                .push(one),
            [] => {}
            _ => stats.ambiguous_client += 1,
        }
    }

    let mut stars = Vec::new();
    for ((_, server), clients) in by_server {
        let keys: Vec<ProjectKey> = clients.iter().map(|p| project(p.client)).collect();
        let mut kept: Vec<&ModelPair> = Vec::new();
        for (p, key) in clients.iter().zip(&keys) {
            if keys.iter().filter(|k| *k == key).count() > 1 {
                stats.ambiguous_project += 1;
            } else {
                kept.push(p);
            }
        }
        if kept.is_empty() {
            continue;
        }
        kept.sort_by_cached_key(|p| {
            (
                p.client_qname.clone(),
                p.joins
                    .first()
                    .map(|j| (j.route_qname.clone(), j.file.clone(), j.line)),
                p.client.0,
            )
        });
        stars.push(ModelStar {
            server: NodeId(server),
            clients: kept,
        });
    }
    (stars, stats)
}

/// Every PROJECT's path (its ORIGIN `path`, `""` for the repo root), per
/// repo. Only looked up, never iterated.
struct ProjectPaths {
    rels: HashMap<RepoId, Vec<String>>,
}

impl ProjectPaths {
    fn of(m: &MergedGraph) -> Self {
        let mut rels: HashMap<RepoId, Vec<String>> = HashMap::new();
        for g in &m.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::PROJECT) {
                    continue;
                }
                let origin = n
                    .cells
                    .iter()
                    .filter(|c| c.kind == cell_type::ORIGIN)
                    .find_map(|c| match &c.payload {
                        CellPayload::Json(j) | CellPayload::Text(j) => str_field(j, "path"),
                        _ => None,
                    });
                let from_qname = || {
                    g.nav
                        .qname_by_id
                        .get(&n.id)
                        .and_then(|q| q.strip_prefix("project:"))
                };
                let Some(rel) = origin.or_else(from_qname) else {
                    continue;
                };
                // The walker names the repo root `project:.`.
                let rel = if rel == "." {
                    ""
                } else {
                    rel.trim_end_matches('/')
                };
                let list = rels.entry(n.repo).or_default();
                if !list.iter().any(|r| r == rel) {
                    list.push(rel.to_string());
                }
            }
        }
        Self { rels }
    }

    /// The deepest PROJECT path of `repo` enclosing the repo-relative `file`:
    /// the root (`""`) encloses every file, any other path only files under
    /// it at a `/` boundary.
    fn owner_of(&self, repo: RepoId, file: &str) -> Option<&str> {
        self.rels
            .get(&repo)?
            .iter()
            .map(String::as_str)
            .filter(|r| {
                r.is_empty()
                    || file
                        .strip_prefix(*r)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by(|a, b| a.len().cmp(&b.len()).then_with(|| b.cmp(a)))
    }
}

/// What one models resolve's `http_model` half found, for its markers.
struct ModelReport {
    client_wires: String,
    wire_pairs: String,
    classes: usize,
    edges: usize,
    diverged: usize,
    stars: StarStats,
}

impl ModelReport {
    fn line(&self) -> String {
        format!(
            "[models] classes={} edges={} diverged={} ambiguous_client={} ambiguous_project={} \
             embed_reduced={}",
            self.classes,
            self.edges,
            self.diverged,
            self.stars.ambiguous_client,
            self.stars.ambiguous_project,
            self.stars.embed_reduced,
        )
    }
}

/// The confidence a SAME_AS edge of an identity rule's tier carries.
fn tier_confidence(tier: IdentityTier) -> Confidence {
    match tier {
        IdentityTier::Fact => Confidence::Strong,
        IdentityTier::Derived => Confidence::Medium,
        IdentityTier::Heuristic => Confidence::Weak,
    }
}

/// The `http_model` SAME_AS edges of `m` and the report behind the markers;
/// `None` when no ENDPOINT hit has an HTTP_CALLS pair.
#[cfg(test)]
fn model_identity(m: &MergedGraph) -> (Vec<Edge>, Option<ModelReport>) {
    model_identity_in(m, &OnceCell::new())
}

/// [`model_identity`] with the resolution context the models resolve shares
/// between its two rules, built on first use.
fn model_identity_in<'m>(
    m: &'m MergedGraph,
    ctx: &OnceCell<Ctx<'m>>,
) -> (Vec<Edge>, Option<ModelReport>) {
    let mut edges = Vec::new();
    if !m
        .cross_edges
        .iter()
        .any(|e| e.category == edge_category::HTTP_CALLS)
    {
        return (edges, None);
    }
    let hits = shaped_hits(m);
    if hits.is_empty() {
        return (edges, None);
    }
    let ctx = ctx.get_or_init(|| Ctx::new(m));
    let cw = wires_in(m, ctx, hits);
    if cw.stats.routed == 0 {
        return (edges, None);
    }
    let wp = pairs_of(m, &cw);
    let mut report = ModelReport {
        client_wires: cw.marker(),
        wire_pairs: wp.marker(),
        classes: 0,
        edges: 0,
        diverged: 0,
        stars: StarStats::default(),
    };
    let Some(rule) = CODE_TABLES.identity_rule(same_as::RULE_HTTP_MODEL) else {
        return (edges, Some(report));
    };
    let confidence = tier_confidence(rule.tier);
    let projects = ProjectPaths::of(m);
    let takes = |p: &ModelPair| ctx.takes(rule, p.client, p.server);
    let mut bases: HashMap<NodeId, BTreeSet<u64>> = HashMap::new();
    for p in wp.model_pairs.iter().filter(|p| takes(p)) {
        bases
            .entry(p.server)
            .or_insert_with(|| ctx.extends_closure(p.server));
    }
    let embeds = |s: NodeId, other: NodeId| bases.get(&s).is_some_and(|b| b.contains(&other.0));
    let project = |c: NodeId| ctx.project_of(m, &projects, c);
    let (stars, stats) = model_stars(&wp.model_pairs, takes, embeds, project);
    report.stars = stats;
    let ev_rule = |diverged: bool| same_as::evidence_rule(rule.name, diverged);
    for star in &stars {
        let mut diverged_class = false;
        for p in &star.clients {
            diverged_class |= p.diverged;
            let mut ev = rule_evidence(PASS, &ev_rule(p.diverged));
            if let Some(j) = p.joins.first() {
                ev = match j.line.checked_sub(1) {
                    Some(line) => ev.at(j.file.clone(), line),
                    None => Evidence {
                        file: Some(j.file.clone()),
                        basis: Basis::File,
                        ..ev
                    },
                };
            }
            edges.push(
                Edge::new(p.client, star.server, edge_category::SAME_AS, confidence)
                    .with_cell(ev.to_cell()),
            );
        }
        report.classes += 1;
        report.diverged += usize::from(diverged_class);
    }
    report.edges = edges.len();
    (edges, Some(report))
}

// ---- DT.1n: GraphQL selection drift ----------------------------------------

/// The emitter of the GRAPHQL_CALLS edges a selection is paired by
/// (graph/src/resolvers/graphql.rs). An overlay `[[edge]]` lands in Post and
/// is not read, so the answer does not depend on when it is asked.
const GRAPHQL_EMITTER: &str = "resolver:graphql";
/// DT.1j's SDL type qname prefix (`message:graphql:<T>`, plus ` @owner`).
const GRAPHQL_TYPE: &str = "message:graphql:";
/// An operation's qname prefix.
const GRAPHQL_OP: &str = "graphql_op:";
/// A root field's qname prefix.
const GRAPHQL_FIELD: &str = "graphql_resolver:";
/// How many selection levels a compare descends: the selection reader's
/// own depth (parsers/code/extractors/src/graphql.rs `MAX_SELECTION_DEPTH`),
/// so it only guards.
const GRAPHQL_MAX_LEVELS: usize = 32;

/// A selected field the schema type does not declare: the server rejects
/// the document at validation, so the operation fails at run time.
const FIELD_UNDEFINED: &str = "graphql_field_undefined";

/// Refused: the op node carries two different selections (two documents
/// under one operation name).
const REFUSED_SELECTION: &str = "conflicting_selection";
/// Refused: the root field carries two different return types.
const REFUSED_RETURNS: &str = "conflicting_returns";
/// Refused: the schema type carries two different field lists.
const REFUSED_FIELDS: &str = "conflicting_fields";
/// Refused: the schema type carries no GraphQL field list.
const REFUSED_NO_FIELDS: &str = "no_fields";
/// Refused: the returned type exists only under another owner (or repo)
/// than the root field's.
const REFUSED_OWNER: &str = "owner_mismatch";
/// Refused: no root selection of the op names the resolver's field (a pair
/// keyed by a folded name).
const REFUSED_ROOT: &str = "root_field_unmatched";

/// A selected field the schema type does not declare.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GraphqlChange {
    /// The schema-field path below the root field (`barcode`,
    /// `owner.email`): each level's schema field, an alias resolved.
    pub path: String,
    /// The response key it was selected under (`code` for `code: barcode`,
    /// else the field).
    pub key: String,
    /// The schema type that lacks it (`Item`).
    pub type_name: String,
    /// `graphql_field_undefined`.
    pub rule: &'static str,
    /// `graphql_field_undefined` breaks the operation.
    pub breaking: bool,
}

/// One GRAPHQL_CALLS operation -> root field pair: the op's selection
/// under that field against the schema type the field returns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GraphqlPair {
    pub op: NodeId,
    pub op_qname: String,
    /// `query` | `mutation` | `subscription`, as the document opens.
    pub op_type: String,
    /// The operation's name as written; an anonymous one's node name.
    pub op_name: String,
    /// The 1-based line of the site that minted the operation.
    pub line: u32,
    pub resolver: NodeId,
    pub resolver_qname: String,
    /// The resolver's LB.8 owner segment, if any: the schema type's too.
    pub owner: Option<String>,
    /// The resolver's field name, which the op's root selection names.
    pub root_field: String,
    /// The response keys the op selects the root field under, sorted (two
    /// for `a: items .. b: items ..`).
    pub response_keys: Vec<String>,
    /// The root field's selection, its response keys' sub-selections merged
    /// ([`Shape::Object`]); a leaf is [`Shape::Open`]. A leaf too when the
    /// pair was refused before the selection was read.
    pub selection: Shape,
    /// The root field's declared return type as written (`[Item!]!`).
    pub returns: String,
    /// The 1-based line of the root field's declaration.
    pub returns_line: u32,
    /// The named type `returns` names, list and non-null wrappers peeled
    /// (`Item`).
    pub type_name: String,
    /// The MESSAGE_TYPE `message:graphql:<type_name>` (+ the resolver's
    /// owner) in the resolver's repo, when one exists.
    pub schema_type: Option<NodeId>,
    pub schema_type_qname: Option<String>,
    /// Sorted by (path, rule).
    pub changes: Vec<GraphqlChange>,
    /// `open`, sorted: a side was open, so no name is claimed absent from it
    /// (an open type, an interface, an open selection, a sub-selected field
    /// whose type names no MESSAGE_TYPE; with no `schema_type`, the root's
    /// own type: a union, a type outside the build).
    pub notes: Vec<&'static str>,
    /// Why nothing was compared: `conflicting_selection`,
    /// `conflicting_returns`, `conflicting_fields`, `no_fields`,
    /// `owner_mismatch`, `root_field_unmatched`.
    pub refused: Option<&'static str>,
}

/// The counts behind [`GraphqlPairs::marker`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct GraphqlPairStats {
    pub pairs: usize,
    /// `graphql_field_undefined` changes over every pair.
    pub undefined_fields: usize,
    /// Of `pairs`: noted `open`.
    pub open: usize,
    /// Of `pairs`: refused.
    pub refused: usize,
}

/// Every [`GraphqlPair`] of a merged graph, sorted by (op qname, resolver
/// qname), with its counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct GraphqlPairs {
    pub pairs: Vec<GraphqlPair>,
    pub stats: GraphqlPairStats,
}

impl GraphqlPairs {
    /// The fired_on line: `[models] graphql pairs=<g> undefined_fields=<u>
    /// open=<o> refused=<r>`.
    pub fn marker(&self) -> String {
        let s = &self.stats;
        format!(
            "[models] graphql pairs={} undefined_fields={} open={} refused={}",
            s.pairs, s.undefined_fields, s.open, s.refused
        )
    }
}

/// Every GRAPHQL_CALLS op -> resolver pair the `graphql` resolver drew,
/// compared selection against schema type (see the module doc's DT.1n
/// part). An op with no selection cell (no document: a codegen hook) or a
/// root field with no return-type cell (a code-first resolver) has nothing
/// to compare and is no pair; neither is a root field returning a type no
/// MESSAGE_TYPE models (a scalar, an enum) that the op selects as a leaf.
pub fn graphql_pairs(merged: &MergedGraph) -> GraphqlPairs {
    if !merged
        .cross_edges
        .iter()
        .any(|e| e.category == edge_category::GRAPHQL_CALLS)
    {
        return GraphqlPairs::default();
    }
    gql_pairs_in(merged, &GqlIndex::new(merged))
}

/// [`graphql_pairs`] over a built index (DT.1k reads the index again).
fn gql_pairs_in(merged: &MergedGraph, ix: &GqlIndex<'_>) -> GraphqlPairs {
    let calls_at = |e: &&Edge| e.category == edge_category::GRAPHQL_CALLS;
    let mut calls: BTreeSet<(&str, &str, u64, u64)> = BTreeSet::new();
    for e in merged.cross_edges.iter().filter(calls_at) {
        if Evidence::of(e).is_some_and(|ev| ev.emitter != GRAPHQL_EMITTER) {
            continue;
        }
        let (Some(op), Some(field)) = (ix.ops.get(&e.from.0), ix.fields.get(&e.to.0)) else {
            continue;
        };
        calls.insert((op.qname, field.qname, e.from.0, e.to.0));
    }
    let mut out = GraphqlPairs::default();
    for (op_qname, field_qname, op, field) in calls {
        if let Some(p) = gql_pair(ix, (op_qname, NodeId(op)), (field_qname, NodeId(field))) {
            out.pairs.push(p);
        }
    }
    let s = &mut out.stats;
    s.pairs = out.pairs.len();
    for p in &out.pairs {
        s.undefined_fields += p
            .changes
            .iter()
            .filter(|c| c.rule == FIELD_UNDEFINED)
            .count();
        s.open += usize::from(p.notes.contains(&NOTE_OPEN));
        s.refused += usize::from(p.refused.is_some());
    }
    out
}

/// One GraphQL node's sites of one form, folded across the graphs that
/// hold it: distinct (raw, shape), each at its least line.
struct GqlNode<'m> {
    qname: &'m str,
    repo: u64,
    sites: Vec<ServerWire>,
}

impl GqlNode<'_> {
    fn add(&mut self, w: ServerWire) {
        match self
            .sites
            .iter_mut()
            .find(|s| s.raw == w.raw && s.shape == w.shape)
        {
            Some(have) => have.line = have.line.min(w.line),
            None => self.sites.push(w),
        }
    }
}

/// One SDL type node, folded across graphs.
#[derive(Default)]
struct GqlType {
    /// Distinct SCHEMA_FIELDS payloads.
    payloads: BTreeSet<String>,
    /// Its ORIGIN says `"decl":"interface"`.
    interface: bool,
}

/// What a type name reaches from a root field's (repo, owner).
enum TypeAt {
    Found {
        id: u64,
        schema: ModelSchema,
        interface: bool,
    },
    /// The node carries two different field lists.
    Conflicting(u64),
    /// The node carries no GraphQL field list.
    NoFields(u64),
    /// A type of that name exists only under another owner or repo.
    Elsewhere,
    /// No type of that name: a scalar, an enum, a union, one outside the
    /// build.
    Missing,
}

/// The GraphQL nodes of a merged graph, built once per [`graphql_pairs`]
/// call; maps are only probed.
struct GqlIndex<'m> {
    /// GRAPHQL_OPERATION id -> its `graphql_selection` sites.
    ops: HashMap<u64, GqlNode<'m>>,
    /// GRAPHQL_RESOLVER id -> its `graphql_returns` sites.
    fields: HashMap<u64, GqlNode<'m>>,
    /// repo -> SDL type qname -> id.
    types: HashMap<u64, HashMap<&'m str, u64>>,
    facts: HashMap<u64, GqlType>,
    /// SDL type id -> its qname.
    type_qname: HashMap<u64, &'m str>,
    /// Every SDL type's bare name (owner dropped).
    names: HashSet<&'m str>,
}

impl<'m> GqlIndex<'m> {
    fn new(m: &'m MergedGraph) -> Self {
        let mut ix = GqlIndex {
            ops: HashMap::new(),
            fields: HashMap::new(),
            types: HashMap::new(),
            facts: HashMap::new(),
            type_qname: HashMap::new(),
            names: HashSet::new(),
        };
        for g in &m.graphs {
            for n in &g.nodes {
                let (Some(&kind), Some(qname)) =
                    (g.nav.kind_by_id.get(&n.id), g.nav.qname_by_id.get(&n.id))
                else {
                    continue;
                };
                let (map, form) = match kind {
                    k if k == node_kind::GRAPHQL_OPERATION => {
                        (&mut ix.ops, WireForm::GraphqlSelection)
                    }
                    k if k == node_kind::GRAPHQL_RESOLVER => {
                        (&mut ix.fields, WireForm::GraphqlReturns)
                    }
                    k if k == node_kind::MESSAGE_TYPE => {
                        let Some(bare) = split_owner(qname).0.strip_prefix(GRAPHQL_TYPE) else {
                            continue;
                        };
                        ix.names.insert(bare);
                        ix.types
                            .entry(n.repo.0)
                            .or_default()
                            .insert(qname.as_str(), n.id.0);
                        ix.type_qname.entry(n.id.0).or_insert(qname.as_str());
                        let t = ix.facts.entry(n.id.0).or_default();
                        for c in &n.cells {
                            let (CellPayload::Json(p) | CellPayload::Text(p)) = &c.payload else {
                                continue;
                            };
                            if c.kind == cell_type::SCHEMA_FIELDS {
                                t.payloads.insert(p.clone());
                            } else if c.kind == cell_type::ORIGIN
                                && str_field(p, "decl") == Some("interface")
                            {
                                t.interface = true;
                            }
                        }
                        continue;
                    }
                    _ => continue,
                };
                let node = map.entry(n.id.0).or_insert_with(|| GqlNode {
                    qname: qname.as_str(),
                    repo: n.repo.0,
                    sites: Vec::new(),
                });
                for w in n
                    .cells
                    .iter()
                    .filter_map(ServerWire::from_cell)
                    .filter(|w| w.form == form)
                {
                    node.add(w);
                }
            }
        }
        // By content, so a refused pair's heading and line do not depend on
        // which graph held which copy.
        for node in ix.ops.values_mut().chain(ix.fields.values_mut()) {
            node.sites
                .sort_by_cached_key(|w| (w.raw.clone(), w.shape.to_json(), w.line));
        }
        ix
    }

    /// The SDL type `name` as the root field of (`repo`, `owner`) reaches
    /// it: `message:graphql:<name>` plus ` @<owner>`, in that repo.
    fn type_at(&self, repo: u64, owner: Option<&str>, name: &str) -> TypeAt {
        if name.is_empty() {
            return TypeAt::Missing;
        }
        let qname = match owner {
            Some(o) => format!("{GRAPHQL_TYPE}{name} @{o}"),
            None => format!("{GRAPHQL_TYPE}{name}"),
        };
        let Some(&id) = self.types.get(&repo).and_then(|t| t.get(qname.as_str())) else {
            return if self.names.contains(name) {
                TypeAt::Elsewhere
            } else {
                TypeAt::Missing
            };
        };
        let Some(t) = self.facts.get(&id) else {
            return TypeAt::NoFields(id);
        };
        let mut payloads = t.payloads.iter();
        match (payloads.next(), payloads.next()) {
            (Some(one), None) => match ModelSchema::parse(one) {
                Some(schema) if schema.format == WireFormat::Graphql => TypeAt::Found {
                    id,
                    schema,
                    interface: t.interface,
                },
                _ => TypeAt::NoFields(id),
            },
            (Some(_), Some(_)) => TypeAt::Conflicting(id),
            (None, _) => TypeAt::NoFields(id),
        }
    }
}

impl GqlIndex<'_> {
    /// DT.1k: the schema type a pair names (its `schema_type`), its fields,
    /// and whether a name may be claimed absent from it (not for an open
    /// type or an interface).
    fn schema_of(&self, p: &GraphqlPair) -> Option<(NodeId, ModelSchema, bool)> {
        let repo = self.fields.get(&p.resolver.0)?.repo;
        match self.type_at(repo, p.owner.as_deref(), &p.type_name) {
            TypeAt::Found {
                id,
                schema,
                interface,
            } if p.schema_type == Some(NodeId(id)) => {
                let closed = !(schema.open || interface);
                Some((NodeId(id), schema, closed))
            }
            _ => None,
        }
    }
}

/// The named type a GraphQL type text names: every list and non-null
/// wrapper peeled (`[[Item!]]!` -> `Item`).
fn graphql_named(ty: &str) -> String {
    let mut t = ty.to_string();
    for _ in 0..GRAPHQL_MAX_LEVELS {
        let (inner, array) = element_type(&t);
        t = inner;
        if !array {
            break;
        }
    }
    t
}

/// `path.name`, or `name` at the root.
fn dotted(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}.{name}")
    }
}

/// `more` merged into `have`: two sub-selections field by field by response
/// key (open when either is), an object over a leaf.
fn merge_selection_into(have: &mut Shape, more: &Shape) {
    if let Shape::Object { fields, open } = have {
        if let Shape::Object {
            fields: extra,
            open: extra_open,
        } = more
        {
            *open |= *extra_open;
            for f in extra {
                match fields.iter_mut().find(|x| x.name == f.name) {
                    Some(x) => {
                        x.required |= f.required;
                        merge_selection_into(&mut x.shape, &f.shape);
                    }
                    None => fields.push(f.clone()),
                }
            }
        }
        return;
    }
    if matches!(more, Shape::Object { .. }) {
        *have = more.clone();
    }
}

/// The pair for one GRAPHQL_CALLS edge; `None` when there is nothing to
/// compare ([`graphql_pairs`]).
fn gql_pair(
    ix: &GqlIndex<'_>,
    (op_qname, op): (&str, NodeId),
    (field_qname, field): (&str, NodeId),
) -> Option<GraphqlPair> {
    let (o, r) = (ix.ops.get(&op.0)?, ix.fields.get(&field.0)?);
    let (sel, ret) = (o.sites.first()?, r.sites.first()?);
    let (base, owner) = split_owner(field_qname);
    let root_field = base.strip_prefix(GRAPHQL_FIELD).unwrap_or(base);
    let mut words = sel.raw.split_whitespace();
    let op_type = words.next().unwrap_or("query");
    let op_name = words.next().map(str::to_string).unwrap_or_else(|| {
        let b = split_owner(op_qname).0;
        b.strip_prefix(GRAPHQL_OP).unwrap_or(b).to_string()
    });
    let type_name = match &ret.shape {
        Shape::Named { ty, .. } => graphql_named(ty),
        _ => String::new(),
    };
    let mut p = GraphqlPair {
        op,
        op_qname: op_qname.to_string(),
        op_type: op_type.to_string(),
        op_name,
        line: sel.line,
        resolver: field,
        resolver_qname: field_qname.to_string(),
        owner: owner.map(str::to_string),
        root_field: root_field.to_string(),
        response_keys: Vec::new(),
        selection: Shape::open(""),
        returns: ret.raw.clone(),
        returns_line: ret.line,
        type_name,
        schema_type: None,
        schema_type_qname: None,
        changes: Vec::new(),
        notes: Vec::new(),
        refused: None,
    };
    if o.sites.len() > 1 {
        p.refused = Some(REFUSED_SELECTION);
        return Some(p);
    }
    if r.sites.len() > 1 {
        p.refused = Some(REFUSED_RETURNS);
        return Some(p);
    }
    let entries: Vec<&ShapeField> = match &sel.shape {
        Shape::Object { fields, .. } => fields
            .iter()
            .filter(|f| f.field.as_deref().unwrap_or(&f.name) == root_field)
            .collect(),
        _ => Vec::new(),
    };
    if entries.is_empty() {
        p.refused = Some(REFUSED_ROOT);
        return Some(p);
    }
    let mut keys: Vec<String> = entries.iter().map(|f| f.name.clone()).collect();
    keys.sort();
    keys.dedup();
    p.response_keys = keys;
    for f in &entries {
        merge_selection_into(&mut p.selection, &f.shape);
    }
    let mut cmp = GqlCompare {
        ix,
        repo: r.repo,
        owner,
        changes: Vec::new(),
        notes: BTreeSet::new(),
    };
    let typed = |p: &mut GraphqlPair, id: u64| {
        p.schema_type = Some(NodeId(id));
        p.schema_type_qname = ix.type_qname.get(&id).map(|q| (*q).to_string());
    };
    match ix.type_at(r.repo, owner, &p.type_name) {
        TypeAt::Found {
            id,
            schema,
            interface,
        } => {
            typed(&mut p, id);
            if let Shape::Object { fields, open } = &p.selection {
                let ty = TypeView {
                    name: &p.type_name,
                    schema: &schema,
                    interface,
                };
                cmp.level(fields, *open, &ty, "", 0);
            }
        }
        TypeAt::Conflicting(id) => {
            typed(&mut p, id);
            p.refused = Some(REFUSED_FIELDS);
            return Some(p);
        }
        TypeAt::NoFields(id) => {
            typed(&mut p, id);
            p.refused = Some(REFUSED_NO_FIELDS);
            return Some(p);
        }
        TypeAt::Elsewhere => {
            p.refused = Some(REFUSED_OWNER);
            return Some(p);
        }
        TypeAt::Missing => {
            if !matches!(p.selection, Shape::Object { .. }) {
                return None;
            }
            cmp.notes.insert(NOTE_OPEN);
        }
    }
    cmp.changes
        .sort_by(|a, b| (&a.path, a.rule).cmp(&(&b.path, b.rule)));
    cmp.changes
        .dedup_by(|a, b| a.path == b.path && a.rule == b.rule);
    p.changes = cmp.changes;
    p.notes = cmp.notes.into_iter().collect();
    Some(p)
}

/// One selection compared level by level with the schema types it reaches.
struct GqlCompare<'i, 'm> {
    ix: &'i GqlIndex<'m>,
    repo: u64,
    owner: Option<&'i str>,
    changes: Vec<GraphqlChange>,
    notes: BTreeSet<&'static str>,
}

/// The schema type one selection level is compared with.
struct TypeView<'a> {
    name: &'a str,
    schema: &'a ModelSchema,
    interface: bool,
}

impl GqlCompare<'_, '_> {
    /// The selected `fields` of one level against type `ty`: a field the
    /// type lacks is undefined unless the type is open or an interface; a
    /// sub-selected field recurses into the type its declared type names,
    /// or notes `open` when that names no SDL type here.
    fn level(
        &mut self,
        fields: &[ShapeField],
        selection_open: bool,
        ty: &TypeView<'_>,
        path: &str,
        depth: usize,
    ) {
        if selection_open {
            self.notes.insert(NOTE_OPEN);
        }
        let claim = !(ty.schema.open || ty.interface);
        if !claim {
            self.notes.insert(NOTE_OPEN);
        }
        for f in fields {
            let field = f.field.as_deref().unwrap_or(&f.name);
            let at = dotted(path, field);
            let Some(declared) = ty.schema.field(field) else {
                if claim {
                    self.changes.push(GraphqlChange {
                        path: at,
                        key: f.name.clone(),
                        type_name: ty.name.to_string(),
                        rule: FIELD_UNDEFINED,
                        breaking: true,
                    });
                }
                continue;
            };
            let Shape::Object { fields: sub, open } = &f.shape else {
                continue;
            };
            if depth + 1 >= GRAPHQL_MAX_LEVELS {
                self.notes.insert(NOTE_OPEN);
                continue;
            }
            let named = graphql_named(&declared.ty);
            match self.ix.type_at(self.repo, self.owner, &named) {
                TypeAt::Found {
                    schema, interface, ..
                } => {
                    let inner = TypeView {
                        name: &named,
                        schema: &schema,
                        interface,
                    };
                    self.level(sub, *open, &inner, &at, depth + 1);
                }
                _ => {
                    self.notes.insert(NOTE_OPEN);
                }
            }
        }
    }
}

// ---- DT.1k: graphql_model identity -----------------------------------------

/// One `graphql_result` site (DT.1k's TS parser cell): a client call that
/// types its result and names its document, on the callable issuing it.
#[derive(Debug, Clone)]
struct GqlResult {
    /// The graph the cell was read from.
    graph: usize,
    from: NodeId,
    /// The callable's repo-relative file.
    file: String,
    /// The call's 1-based line.
    line: u32,
    document: String,
    shape: Shape,
}

/// Every `graphql_result` MESSAGE_TYPE site of `m` that names a document,
/// an identical (node, payload) once across graphs, sorted by (file, line,
/// document, payload).
fn graphql_results(m: &MergedGraph, ctx: &Ctx<'_>) -> Vec<GqlResult> {
    let mut seen: BTreeSet<(u64, &str)> = BTreeSet::new();
    let mut out: Vec<(GqlResult, &str)> = Vec::new();
    for (gi, g) in m.graphs.iter().enumerate() {
        for n in &g.nodes {
            for c in n.cells.iter().filter(|c| c.kind == cell_type::MESSAGE_TYPE) {
                let (CellPayload::Json(p) | CellPayload::Text(p)) = &c.payload else {
                    continue;
                };
                if !p.contains("\"form\":\"graphql_result\"") || !seen.insert((n.id.0, p.as_str()))
                {
                    continue;
                }
                let Some(w) = ServerWire::parse(p).filter(|w| w.form == WireForm::GraphqlResult)
                else {
                    continue;
                };
                let Some(document) = w.document else {
                    continue;
                };
                let file = ctx.idx[gi].file_of.get(&n.id).cloned().unwrap_or_default();
                let r = GqlResult {
                    graph: gi,
                    from: n.id,
                    file,
                    line: w.line,
                    document,
                    shape: w.shape,
                };
                out.push((r, p.as_str()));
            }
        }
    }
    out.sort_by(|(a, pa), (b, pb)| {
        (&a.file, a.line, &a.document, pa).cmp(&(&b.file, b.line, &b.document, pb))
    });
    out.into_iter().map(|(r, _)| r).collect()
}

/// Every POSITION cell of `cells` as (file, first row, last row): the file
/// as the payload writes it (both sides of a comparison read it the same
/// way), a missing `end_line` the start row.
fn position_spans(cells: &[Cell]) -> Vec<(&str, u32, u32)> {
    cells
        .iter()
        .filter(|c| c.kind == cell_type::POSITION)
        .filter_map(|c| {
            let (CellPayload::Json(j) | CellPayload::Text(j)) = &c.payload else {
                return None;
            };
            let file = str_field(j, "file").filter(|f| !f.is_empty())?;
            let start = num_field(j, "start_line")?;
            let end = num_field(j, "end_line").unwrap_or(start).max(start);
            Some((file, start, end))
        })
        .collect()
}

/// The unsigned integer under `key` of a compact JSON payload.
fn num_field(json: &str, key: &str) -> Option<u32> {
    let pat = format!("\"{key}\":");
    let rest = &json[json.find(&pat)? + pat.len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Where each GRAPHQL_OPERATION was minted: (repo, file) -> its (row, op)
/// sites, from every POSITION cell of every op node. Only probed.
fn operation_sites(m: &MergedGraph) -> HashMap<(u64, &str), Vec<(u32, u64)>> {
    let mut out: HashMap<(u64, &str), Vec<(u32, u64)>> = HashMap::new();
    for g in &m.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::GRAPHQL_OPERATION) {
                continue;
            }
            for (file, start, _) in position_spans(&n.cells) {
                let at = out.entry((n.repo.0, file)).or_default();
                if !at.contains(&(start, n.id.0)) {
                    at.push((start, n.id.0));
                }
            }
        }
    }
    out
}

/// A struct's file is generated code, the way the provenance pass stamps
/// ORIGIN `generated` (DE.7): a codegen file name, or a generator header on
/// the file's MODULE CODE cell; a protobuf name (ORIGIN `generated_proto`)
/// is not. An ORIGIN `generated` the node already carries (a layout merge
/// reads persisted cells) counts too. The models pass runs before that post
/// pass, so it reads the same evidence itself.
fn is_generated_struct(ix: &GraphIndex<'_>, node: &glia_core::Node) -> bool {
    let stamped = node.cells.iter().any(|c| {
        c.kind == cell_type::ORIGIN
            && matches!(&c.payload, CellPayload::Json(p) | CellPayload::Text(p)
                if str_field(p, "provenance") == Some("generated"))
    });
    if stamped {
        return true;
    }
    let Some(file) = ix.file_of.get(&node.id) else {
        return false;
    };
    match generated_by_name(file) {
        Some(Generated::Codegen) => return true,
        Some(Generated::Proto) => return false,
        None => {}
    }
    let Some(module) = ix
        .module_by_file
        .get(file)
        .and_then(|m| ix.at.get(m))
        .and_then(|i| ix.g.nodes.get(*i))
    else {
        return false;
    };
    module.cells.iter().any(|c| {
        c.kind == cell_type::CODE
            && matches!(&c.payload, CellPayload::Text(code) if generated_by_banner(code))
    })
}

/// Every generated STRUCT with a `go` SCHEMA_FIELDS model
/// ([`is_generated_struct`]): (repo, name) -> its (graph, id), first graph
/// holding it, in graph and node order. Only probed.
fn generated_structs<'m>(
    m: &'m MergedGraph,
    ctx: &Ctx<'_>,
) -> HashMap<(u64, &'m str), Vec<(usize, NodeId)>> {
    let mut out: HashMap<(u64, &str), Vec<(usize, NodeId)>> = HashMap::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    for (gi, g) in m.graphs.iter().enumerate() {
        let ix = &ctx.idx[gi];
        for n in &g.nodes {
            if ix.kind(n.id) != Some(node_kind::STRUCT)
                || ix
                    .schemas
                    .get(&n.id)
                    .is_none_or(|s| s.format != WireFormat::Go)
                || !seen.insert(n.id)
                || !is_generated_struct(ix, n)
            {
                continue;
            }
            let name = g.nav.name_by_id.get(&n.id).map_or("", String::as_str);
            out.entry((n.repo.0, name)).or_default().push((gi, n.id));
        }
    }
    out
}

/// What the `graphql_model` half of one models resolve found, for its
/// marker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct GraphqlModelReport {
    /// `graphql_result` sites naming a document.
    results: usize,
    /// Of `results`: the document bound to a STATE_VAR.
    documents_resolved: usize,
    /// Classes a generated struct joined.
    generated_models: usize,
    classes: usize,
}

impl GraphqlModelReport {
    fn line(&self) -> String {
        format!(
            "[models] graphql results={} documents_resolved={} generated_models={} classes={}",
            self.results, self.documents_resolved, self.generated_models, self.classes
        )
    }
}

/// One client result key that reached a schema type and confirmed it.
struct GqlJoin {
    client: NodeId,
    center: NodeId,
    join: ModelJoin,
}

/// The `graphql_model` SAME_AS edges of `m` (see [`graphql_model_identity_in`]).
#[cfg(test)]
fn graphql_model_identity(m: &MergedGraph) -> (Vec<Edge>, Option<GraphqlModelReport>) {
    graphql_model_identity_in(m, &OnceCell::new())
}

/// DT.1k: the `graphql_model` classes. `None` when no callable carries a
/// `graphql_result` site naming a document; otherwise the report, with the
/// edges when GRAPHQL_CALLS pairs reach a schema type.
///
/// 1. **The document.** A site's `document` binds in the callable's MODULE
///    through [`bind_client_name`] (own symbols, import bindings with TS
///    re-export barrels, `ns.NAME`), any kind, and is kept only when it is a
///    STATE_VAR (`documents_resolved`).
/// 2. **The operation.** The one GRAPHQL_OPERATION of the STATE_VAR's repo
///    minted in its POSITION span (an op POSITION in that file, its row in
///    the const's rows). None, or two (two documents in one const), joins
///    nothing.
/// 3. **The schema type.** The result type resolves as a client value in
///    the callable's module ([`Ctx::resolve`]); each root key k of it (an
///    object, a model's fields, or an all-object union's merged fields) is
///    a response key of the op's selection, whose `field` (an alias's schema
///    field, else k) is a root field f. The op's [`GraphqlPair`] with root
///    field f, not refused, names the schema type T (DT.1n's lookup:
///    `message:graphql:<T>` plus the resolver's owner, in its repo); two
///    pairs naming two types join nothing.
/// 4. **The client model.** The value at k, when it resolved to a client
///    model, pairs T when [`confirms`] holds between the model's JSON names
///    and T's field names (no case folding), diverged when the model lists a
///    name T does not (an open or interface T claims none absent). A TS type
///    alias (graphql-codegen's `type GetUsersQuery = {..}`) mints no node:
///    its keys are unknown and no class forms.
/// 5. **Stars.** [`model_stars`] over the pairs (no embed rule): a client
///    model pairing two types joins neither, two client models of one
///    project pairing one type are both dropped, one per project joins. The
///    center is T. A star's generated model then joins it: the one STRUCT
///    of T's repo and owning project whose name is T's, generated
///    ([`is_generated_struct`]) and whose `go` field names confirm T's
///    (two such structs: none), diverged when the two name sets differ.
///    gqlgen's hand-written domain structs of the same name are never
///    generated, so they never join.
///
/// Edges: member -> T, SAME_AS, the `graphql_model` row's tier (DERIVED,
/// Medium), one EVIDENCE cell `resolver:models` rule `graphql_model` or
/// `graphql_model/diverged`, located at the first joining call site (joins
/// sorted by op qname, file, line), a generated struct at its own
/// declaration.
fn graphql_model_identity_in<'m>(
    m: &'m MergedGraph,
    ctx: &OnceCell<Ctx<'m>>,
) -> (Vec<Edge>, Option<GraphqlModelReport>) {
    let mut edges = Vec::new();
    let any_result = m.graphs.iter().any(|g| {
        g.nodes.iter().any(|n| {
            n.cells.iter().any(|c| {
                c.kind == cell_type::MESSAGE_TYPE
                    && matches!(&c.payload, CellPayload::Json(p) | CellPayload::Text(p)
                        if p.contains("\"form\":\"graphql_result\""))
            })
        })
    });
    if !any_result {
        return (edges, None);
    }
    let ctx = ctx.get_or_init(|| Ctx::new(m));
    let results = graphql_results(m, ctx);
    if results.is_empty() {
        return (edges, None);
    }
    let mut report = GraphqlModelReport {
        results: results.len(),
        ..GraphqlModelReport::default()
    };
    let has_calls = m
        .cross_edges
        .iter()
        .any(|e| e.category == edge_category::GRAPHQL_CALLS);
    let gix = GqlIndex::new(m);
    let gp = if has_calls {
        gql_pairs_in(m, &gix)
    } else {
        GraphqlPairs::default()
    };
    let mut pairs_of_op: HashMap<u64, Vec<&GraphqlPair>> = HashMap::new();
    for p in gp
        .pairs
        .iter()
        .filter(|p| p.refused.is_none() && p.schema_type.is_some())
    {
        pairs_of_op.entry(p.op.0).or_default().push(p);
    }
    let ops = operation_sites(m);

    let mut joins: Vec<GqlJoin> = Vec::new();
    // Each reached schema type's bare name and field names.
    let mut centers: HashMap<u64, (String, BTreeSet<String>)> = HashMap::new();
    for r in &results {
        let Some(module) = ctx.module_of(r.graph, r.from) else {
            continue;
        };
        let Some(doc) = bind_client_name(ctx.idx[r.graph].g, module, &r.document) else {
            continue;
        };
        let Some(dg) = ctx.graph_of(doc, r.graph) else {
            continue;
        };
        let dix = &ctx.idx[dg];
        if dix.kind(doc) != Some(node_kind::STATE_VAR) {
            continue;
        }
        report.documents_resolved += 1;
        let Some(node) = dix.at.get(&doc).and_then(|i| dix.g.nodes.get(*i)) else {
            continue;
        };
        let mut minted: BTreeSet<u64> = BTreeSet::new();
        for (file, start, end) in position_spans(&node.cells) {
            for (row, op) in ops.get(&(node.repo.0, file)).into_iter().flatten() {
                if (start..=end).contains(row) {
                    minted.insert(*op);
                }
            }
        }
        let (Some(&op), None) = (minted.first(), minted.iter().nth(1)) else {
            continue;
        };
        let Some(sel) = gix
            .ops
            .get(&op)
            .filter(|o| o.sites.len() == 1)
            .and_then(|o| o.sites.first())
        else {
            continue;
        };
        let Shape::Object {
            fields: selected, ..
        } = &sel.shape
        else {
            continue;
        };
        let format = format_for_file(&r.file);
        let names = Names::Client(Some((r.graph, module)));
        let resolved = ctx.resolve(r.graph, names, &r.shape, format, WireDirection::Response, 0);
        let Some(root) = object_view(&resolved) else {
            continue;
        };
        for (key, field) in &root.fields {
            let Some(client) = field.value.model else {
                continue;
            };
            let Some(entry) = selected.iter().find(|f| f.name == *key) else {
                continue;
            };
            let root_field = entry.field.as_deref().unwrap_or(&entry.name);
            let reached: Vec<&GraphqlPair> = pairs_of_op
                .get(&op)
                .into_iter()
                .flatten()
                .copied()
                .filter(|p| p.root_field == root_field)
                .collect();
            let types: BTreeSet<u64> = reached
                .iter()
                .filter_map(|p| p.schema_type)
                .map(|t| t.0)
                .collect();
            let (Some(&pair), 1) = (reached.first(), types.len()) else {
                continue;
            };
            let Some((center, schema, closed)) = gix.schema_of(pair) else {
                continue;
            };
            let names_of_center = schema.fields.iter().map(|f| f.name.clone()).collect();
            centers
                .entry(center.0)
                .or_insert_with(|| (pair.type_name.clone(), names_of_center));
            let server: BTreeSet<&str> = schema.fields.iter().map(|f| f.name.as_str()).collect();
            let client_names: BTreeSet<&str> =
                field.value.fields.keys().map(String::as_str).collect();
            let Some(overlap) =
                confirms(client_names.iter().copied(), server.iter().copied(), false)
            else {
                continue;
            };
            let diverged = closed && client_names.iter().any(|n| !server.contains(n));
            joins.push(GqlJoin {
                client,
                center,
                join: ModelJoin {
                    direction: WireDirection::Response,
                    route_qname: pair.op_qname.clone(),
                    file: r.file.clone(),
                    line: r.line,
                    handler_qname: pair.resolver_qname.clone(),
                    path: key.clone(),
                    overlap,
                    client_fields: client_names.len(),
                    server_fields: server.len(),
                    diverged,
                },
            });
        }
    }
    let Some(rule) = CODE_TABLES.identity_rule(same_as::RULE_GRAPHQL_MODEL) else {
        return (edges, Some(report));
    };
    if joins.is_empty() {
        return (edges, Some(report));
    }

    // One ModelPair per (client, type), joins sorted as DT.1h sorts them.
    let mut index: HashMap<(NodeId, NodeId), usize> = HashMap::new();
    let mut model_pairs: Vec<ModelPair> = Vec::new();
    for j in joins {
        let at = *index.entry((j.client, j.center)).or_insert_with(|| {
            model_pairs.push(ModelPair {
                client: j.client,
                client_qname: qname_of(m, j.client),
                server: j.center,
                server_qname: qname_of(m, j.center),
                overlap: 0,
                diverged: false,
                joins: Vec::new(),
            });
            model_pairs.len() - 1
        });
        if let Some(mp) = model_pairs.get_mut(at) {
            mp.overlap = mp.overlap.max(j.join.overlap);
            mp.diverged |= j.join.diverged;
            if !mp.joins.contains(&j.join) {
                mp.joins.push(j.join);
            }
        }
    }
    let join_key = |j: &ModelJoin| {
        (
            j.route_qname.clone(),
            j.file.clone(),
            j.line,
            j.handler_qname.clone(),
            j.path.clone(),
        )
    };
    for mp in &mut model_pairs {
        mp.joins.sort_by_cached_key(join_key);
    }
    model_pairs.sort_by_cached_key(|mp| {
        (
            mp.client_qname.clone(),
            mp.server_qname.clone(),
            mp.joins.first().map(join_key),
        )
    });

    let confidence = tier_confidence(rule.tier);
    let projects = ProjectPaths::of(m);
    let takes = |p: &ModelPair| ctx.takes(rule, p.client, p.server);
    let project = |c: NodeId| ctx.project_of(m, &projects, c);
    let (stars, _) = model_stars(&model_pairs, takes, |_, _| false, project);
    let generated = generated_structs(m, ctx);
    let ev_rule = |diverged: bool| same_as::evidence_rule(rule.name, diverged);
    for star in &stars {
        for p in &star.clients {
            let mut ev = rule_evidence(PASS, &ev_rule(p.diverged));
            if let Some(j) = p.joins.first() {
                ev = match j.line.checked_sub(1) {
                    Some(line) => ev.at(j.file.clone(), line),
                    None => Evidence {
                        file: Some(j.file.clone()),
                        basis: Basis::File,
                        ..ev
                    },
                };
            }
            edges.push(
                Edge::new(p.client, star.server, edge_category::SAME_AS, confidence)
                    .with_cell(ev.to_cell()),
            );
        }
        report.classes += 1;
        let Some((model, diverged, at)) = centers
            .get(&star.server.0)
            .and_then(|c| generated_model(ctx, &projects, &generated, star.server, c))
        else {
            continue;
        };
        if !ctx.takes(rule, model, star.server) {
            continue;
        }
        let ev = rule_evidence(PASS, &ev_rule(diverged));
        let ev = match at {
            Some((file, Some(line))) => ev.at(file, line),
            Some((file, None)) => Evidence {
                file: Some(file),
                basis: Basis::File,
                ..ev
            },
            None => ev,
        };
        edges.push(
            Edge::new(model, star.server, edge_category::SAME_AS, confidence)
                .with_cell(ev.to_cell()),
        );
        report.generated_models += 1;
    }
    (edges, Some(report))
}

/// The generated model of the schema type `center` ([`graphql_model_identity_in`]
/// step 5), `(name, field names)` the type's: the struct, whether its names
/// differ from the type's, and its declaration's location.
fn generated_model(
    ctx: &Ctx<'_>,
    projects: &ProjectPaths,
    generated: &HashMap<(u64, &str), Vec<(usize, NodeId)>>,
    center: NodeId,
    (name, server): &(String, BTreeSet<String>),
) -> Option<(NodeId, bool, Option<Location>)> {
    let tg = ctx.graphs(center).first().copied()?;
    let tix = &ctx.idx[tg];
    let repo = tix.g.nodes.get(*tix.at.get(&center)?)?.repo;
    let owner = tix
        .file_of
        .get(&center)
        .and_then(|f| projects.owner_of(repo, f));
    let mut found: Vec<(NodeId, bool, Option<Location>)> = Vec::new();
    for &(gi, id) in generated
        .get(&(repo.0, name.as_str()))
        .into_iter()
        .flatten()
    {
        let ix = &ctx.idx[gi];
        let (Some(file), Some(fields)) = (ix.file_of.get(&id), ix.schemas.get(&id)) else {
            continue;
        };
        if projects.owner_of(repo, file) != owner {
            continue;
        }
        let names: BTreeSet<String> = fields.fields.iter().map(|f| f.name.clone()).collect();
        let shared = (
            names.iter().map(String::as_str),
            server.iter().map(String::as_str),
        );
        if confirms(shared.0, shared.1, false).is_none() {
            continue;
        }
        let at = ix
            .at
            .get(&id)
            .and_then(|i| ix.g.nodes.get(*i))
            .and_then(|n| locate(&n.cells));
        found.push((id, names != *server, at));
    }
    match found.as_slice() {
        [one] => Some(one.clone()),
        _ => None,
    }
}

// ---- DT.1o: one side flattened ---------------------------------------------

/// One JSON name of a flattened side ([`WireFields`]), at its path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WireField {
    /// The JSON name (a GraphQL response key); empty at the root.
    pub name: String,
    /// The source member: a Go field, a TS / Dart property (a `@JsonKey`
    /// rename's member), a GraphQL schema field; the root's own name at the
    /// root.
    pub member: String,
    /// Always present on the wire (a GraphQL field: non-null; a selection:
    /// always).
    pub required: bool,
    /// A request binder rejects a body without it.
    pub bind_required: bool,
    /// `string` | `number` | `boolean` | `array` | `object` | `any` | `null`
    /// ([`json_kind`] of the declared text); `None` when no kind is known: a
    /// name that resolved to no model, a call no builder typed, a selection.
    pub kind: Option<&'static str>,
    /// The value is an array of its element (one layer).
    pub array: bool,
    /// The declared type text, as written; else the value as written; empty
    /// for a selection.
    pub ty: String,
    /// The model (a GraphQL MESSAGE_TYPE) the value resolved to.
    pub model: Option<NodeId>,
    /// The node whose SCHEMA_FIELDS declares the name: the model, or the
    /// `extends` base it was flattened from. `None` at the root, for an
    /// inline object's key and for a selection.
    pub declared_by: Option<NodeId>,
}

/// One side of an exchange flattened to JSON paths: the root is `""`, a
/// field `name`, a nested model's field `owner.email`, an array's element
/// field `lines[].productId` (a root array's element fields unprefixed:
/// `id`); a GraphQL side keys the schema-field path (`owner.email`, no `[]`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct WireFields {
    pub fields: BTreeMap<String, WireField>,
    /// Paths below which names may travel that `fields` does not list: an
    /// object / any value with no model, an open model, union or shape, a
    /// model met again on its own path; `""` when the whole side is open.
    pub open: BTreeSet<String>,
}

/// The kind name of `k`; an unknown name is an object only when it resolved
/// to a model.
fn kind_name(k: &JsonKind, model: bool) -> Option<&'static str> {
    match k {
        JsonKind::Unknown(_) => model.then_some(JsonKind::Object.as_str()),
        k => Some(k.as_str()),
    }
}

/// The kind a value takes at its position: an array when one layer was
/// peeled; else its declared text's; else its element's.
fn wire_kind(declared: Option<&str>, v: &ResolvedShape) -> Option<&'static str> {
    if v.array {
        return Some(JsonKind::Array.as_str());
    }
    match declared {
        Some(d) => kind_name(&kind_of(v.format, d), v.model.is_some()),
        None => element_wire_kind(v),
    }
}

/// The kind of `v`'s element (`v` itself when it is no array): a model or an
/// inline object is an object, a written name its format's kind, a union the
/// kind its members share; a call or chain no builder typed and an open
/// value have none.
fn element_wire_kind(v: &ResolvedShape) -> Option<&'static str> {
    if v.model.is_some() {
        return Some(JsonKind::Object.as_str());
    }
    match &v.written {
        Shape::Named { ty, .. } => kind_name(&kind_of(v.format, ty), false),
        Shape::Object { .. } => Some(JsonKind::Object.as_str()),
        Shape::Union(_) => {
            let mut kinds = v.members.iter().map(|m| wire_kind(None, m));
            let first = kinds.next()??;
            kinds.all(|k| k == Some(first)).then_some(first)
        }
        Shape::Call { .. } | Shape::Chain { .. } | Shape::Open { .. } => None,
    }
}

/// One side of an HTTP exchange ([`ClientWire::client`] or a
/// [`ServerSite::shape`]) flattened to its JSON paths (see [`WireFields`]).
/// The shape was resolved for its direction already: a model's custom
/// `MarshalJSON` / `UnmarshalJSON`, its open schema or an unbound base made
/// it open, and nested models expand to `MAX_DEPTH` (4) levels. A model met
/// again on its own path is not re-entered (its path is open). No map is
/// iterated into the output: both maps are sorted.
pub fn wire_fields(side: &ResolvedShape) -> WireFields {
    let mut out = WireFields::default();
    out.fields.insert(
        String::new(),
        WireField {
            required: true,
            kind: wire_kind(None, side),
            array: side.array,
            ty: type_text(None, side),
            model: side.model,
            ..WireField::default()
        },
    );
    let mut on_path: Vec<NodeId> = Vec::new();
    flatten_into(side, "", true, &mut on_path, &mut out);
    out
}

/// The names below the value `v` held at `path` (its own entry already
/// listed), and the open paths it marks.
fn flatten_into(
    v: &ResolvedShape,
    path: &str,
    root: bool,
    on_path: &mut Vec<NodeId>,
    out: &mut WireFields,
) {
    if v.open {
        out.open.insert(path.to_string());
    }
    if matches!(v.written, Shape::Union(_)) {
        out.open.insert(path.to_string());
        return;
    }
    let expanded = v.model.is_some() || matches!(v.written, Shape::Object { .. });
    if !expanded {
        // A value whose names are unknown: an object or any value no model
        // lists, a name that bound nothing.
        if matches!(element_wire_kind(v), None | Some("object") | Some("any")) {
            out.open.insert(path.to_string());
        }
        return;
    }
    if let Some(m) = v.model {
        if on_path.contains(&m) {
            out.open.insert(path.to_string());
            return;
        }
        on_path.push(m);
    }
    let prefix = if v.array && !root {
        format!("{path}[]")
    } else {
        path.to_string()
    };
    for (name, f) in &v.fields {
        let at = join_path(&prefix, name);
        out.fields.insert(
            at.clone(),
            WireField {
                name: name.clone(),
                member: f.member.clone().unwrap_or_else(|| name.clone()),
                required: f.required,
                bind_required: f.bind_required,
                kind: wire_kind(f.declared.as_deref(), &f.value),
                array: f.value.array,
                ty: type_text(f.declared.as_deref(), &f.value),
                model: f.value.model,
                declared_by: f.declared_in,
            },
        );
        flatten_into(&f.value, &at, false, on_path, out);
    }
    if v.model.is_some() {
        on_path.pop();
    }
}

/// One GraphQL pair's two sides flattened by schema-field path: `(selection,
/// served)`. The selection is the op's root entry for the pair's root field
/// (its response keys' sub-selections merged, [`GraphqlPair::selection`]):
/// each selected field keyed by its schema field (an alias's `field`), `name`
/// its response key, `member` the schema field, always required, no kind; an
/// open selection level (a spread, an inline fragment) is an open path. The
/// served side is the schema type the root field returns: its SCHEMA_FIELDS
/// (format graphql, required = non-null), each declared by the type's
/// MESSAGE_TYPE, recursing into a MESSAGE_TYPE-typed field of the same owner
/// where the selection sub-selects it (as [`graphql_pairs`] compares); an
/// open type or interface, and a sub-selected field whose type names no
/// MESSAGE_TYPE here, are open paths, and a pair with no schema type is open
/// at `""`. One index per call: [`graphql_fields_all`] reads many pairs.
pub fn graphql_fields(merged: &MergedGraph, pair: &GraphqlPair) -> (WireFields, WireFields) {
    graphql_fields_all(merged, std::slice::from_ref(pair))
        .pop()
        .unwrap_or_default()
}

/// [`graphql_fields`] of every pair in `pairs`, in order, over one index.
pub fn graphql_fields_all(
    merged: &MergedGraph,
    pairs: &[GraphqlPair],
) -> Vec<(WireFields, WireFields)> {
    if pairs.is_empty() {
        return Vec::new();
    }
    let ix = GqlIndex::new(merged);
    pairs.iter().map(|p| gql_fields(&ix, p)).collect()
}

fn gql_fields(ix: &GqlIndex<'_>, p: &GraphqlPair) -> (WireFields, WireFields) {
    let mut sel = WireFields::default();
    sel.fields.insert(
        String::new(),
        WireField {
            name: p
                .response_keys
                .first()
                .cloned()
                .unwrap_or_else(|| p.root_field.clone()),
            member: p.root_field.clone(),
            required: true,
            ..WireField::default()
        },
    );
    selection_into(&p.selection, "", 0, &mut sel);

    let mut served = WireFields::default();
    let repo = ix.fields.get(&p.resolver.0).map(|r| r.repo);
    served.fields.insert(
        String::new(),
        WireField {
            name: p.root_field.clone(),
            member: p.root_field.clone(),
            required: p.returns.trim_end().ends_with('!'),
            kind: kind_name(
                &json_kind(WireFormat::Graphql, &p.returns),
                p.schema_type.is_some(),
            ),
            array: element_type(&p.returns).1,
            ty: p.returns.clone(),
            model: p.schema_type,
            ..WireField::default()
        },
    );
    let found = repo.map(|r| (r, ix.type_at(r, p.owner.as_deref(), &p.type_name)));
    match found {
        Some((
            r,
            TypeAt::Found {
                id,
                schema,
                interface,
            },
        )) if p.schema_type == Some(NodeId(id)) => {
            let ty = ServedType {
                id,
                schema: &schema,
                interface,
            };
            let at = ServedAt {
                ix,
                repo: r,
                owner: p.owner.as_deref(),
            };
            at.served_into(&ty, &p.selection, "", 0, &mut served);
        }
        _ => {
            served.open.insert(String::new());
        }
    }
    (sel, served)
}

/// A selection's fields below `prefix`, by schema-field path.
fn selection_into(sel: &Shape, prefix: &str, depth: usize, out: &mut WireFields) {
    let Shape::Object { fields, open } = sel else {
        return;
    };
    if *open {
        out.open.insert(prefix.to_string());
    }
    for f in fields {
        let member = f.field.as_deref().unwrap_or(&f.name);
        let at = dotted(prefix, member);
        out.fields.entry(at.clone()).or_insert_with(|| WireField {
            name: f.name.clone(),
            member: member.to_string(),
            required: true,
            ..WireField::default()
        });
        if depth + 1 < GRAPHQL_MAX_LEVELS {
            selection_into(&f.shape, &at, depth + 1, out);
        } else if matches!(f.shape, Shape::Object { .. }) {
            out.open.insert(at);
        }
    }
}

/// The sub-selection a selection level gives schema field `field`: every
/// response key's merged ([`merge_selection_into`]); `None` for a leaf.
fn sub_selection(sel: &Shape, field: &str) -> Option<Shape> {
    let Shape::Object { fields, .. } = sel else {
        return None;
    };
    let mut out: Option<Shape> = None;
    for f in fields
        .iter()
        .filter(|f| f.field.as_deref().unwrap_or(&f.name) == field)
    {
        match &mut out {
            None => out = Some(f.shape.clone()),
            Some(have) => merge_selection_into(have, &f.shape),
        }
    }
    out.filter(|s| matches!(s, Shape::Object { .. }))
}

/// Where a served side's types are looked up: the root field's repo and owner.
struct ServedAt<'i, 'm> {
    ix: &'i GqlIndex<'m>,
    repo: u64,
    owner: Option<&'i str>,
}

/// One schema type a served level lists.
struct ServedType<'a> {
    id: u64,
    schema: &'a ModelSchema,
    interface: bool,
}

impl ServedAt<'_, '_> {
    /// `ty`'s fields below `prefix`, recursing where `sel` sub-selects.
    fn served_into(
        &self,
        ty: &ServedType<'_>,
        sel: &Shape,
        prefix: &str,
        depth: usize,
        out: &mut WireFields,
    ) {
        if ty.schema.open || ty.interface {
            out.open.insert(prefix.to_string());
        }
        for f in &ty.schema.fields {
            let at = dotted(prefix, &f.name);
            let named = graphql_named(&f.ty);
            let found = self.ix.type_at(self.repo, self.owner, &named);
            let model = match &found {
                TypeAt::Found { id, .. } => Some(NodeId(*id)),
                _ => None,
            };
            out.fields.insert(
                at.clone(),
                WireField {
                    name: f.name.clone(),
                    member: f.member().to_string(),
                    required: f.required,
                    bind_required: false,
                    kind: kind_name(&json_kind(WireFormat::Graphql, &f.ty), model.is_some()),
                    array: f.array,
                    ty: f.ty.clone(),
                    model,
                    declared_by: Some(NodeId(ty.id)),
                },
            );
            let Some(sub) = sub_selection(sel, &f.name) else {
                continue;
            };
            match found {
                TypeAt::Found {
                    id,
                    schema,
                    interface,
                } if depth + 1 < GRAPHQL_MAX_LEVELS => {
                    let inner = ServedType {
                        id,
                        schema: &schema,
                        interface,
                    };
                    self.served_into(&inner, &sub, &at, depth + 1, out);
                }
                _ => {
                    out.open.insert(at);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::wire::{ModelField, ShapeField, append_hit_fields};
    use glia_code_domain::{CodeNav, GRAPH_TYPE};
    use glia_core::{Cell, Confidence, Edge, EdgeCategoryId, Node, RepoId};

    /// A hand-built `RepoGraph`: nodes with cells, intra edges, symbols.
    struct G {
        g: RepoGraph,
    }

    impl G {
        fn new(tag: &str) -> Self {
            let repo = RepoId::from_canonical(&format!("test://{tag}"));
            G {
                g: RepoGraph {
                    repo,
                    nodes: vec![],
                    edges: vec![],
                    symbols: Default::default(),
                    nav: CodeNav::default(),
                    unresolved_calls: vec![],
                    unresolved_refs: vec![],
                    properties: Default::default(),
                },
            }
        }

        /// A node `qname` (name = its last `::` segment) under `parent`.
        fn node(
            &mut self,
            kind: NodeKindId,
            qname: &str,
            parent: Option<NodeId>,
            cells: Vec<Cell>,
        ) -> NodeId {
            let id = NodeId::from_parts(GRAPH_TYPE, self.g.repo, kind, qname);
            let name = qname.rsplit("::").next().unwrap_or(qname);
            self.g.nav.record(id, name, qname, kind, parent);
            self.g.nodes.push(Node {
                id,
                repo: self.g.repo,
                confidence: Confidence::Strong,
                cells,
            });
            id
        }

        fn module(&mut self, qname: &str, file: &str) -> NodeId {
            self.node(node_kind::MODULE, qname, None, vec![pos(file)])
        }

        /// `cell` added to the node `id`.
        fn cell(&mut self, id: NodeId, cell: Cell) {
            if let Some(n) = self.g.nodes.iter_mut().find(|n| n.id == id) {
                n.cells.push(cell);
            }
        }

        fn edge(&mut self, from: NodeId, to: NodeId, category: EdgeCategoryId) {
            self.g
                .edges
                .push(Edge::new(from, to, category, Confidence::Strong));
        }

        fn symbol(&mut self, module: NodeId, name: &str, id: NodeId) {
            self.g
                .symbols
                .module_symbols
                .entry(module)
                .or_default()
                .insert(name.to_string(), id);
        }

        fn import(&mut self, module: NodeId, name: &str, id: NodeId) {
            self.g
                .symbols
                .module_import_bindings
                .entry(module)
                .or_default()
                .insert(name.to_string(), id);
        }

        fn method(&mut self, owner: NodeId, name: &str) {
            let q = format!("{}::{name}", self.g.nav.qname_by_id[&owner]);
            let id = self.node(node_kind::METHOD, &q, Some(owner), vec![]);
            self.g
                .symbols
                .class_methods
                .entry(owner)
                .or_default()
                .insert(name.to_string(), id);
        }
    }

    fn pos(file: &str) -> Cell {
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(r#"{{"file":"{file}","start_line":0}}"#)),
        }
    }

    fn schema(format: WireFormat, fields: &[(&str, &str, bool)], extends: &[&str]) -> Cell {
        let mut s = ModelSchema::new(format);
        s.fields = fields
            .iter()
            .map(|(n, t, r)| ModelField::new(n, t, *r))
            .collect();
        s.extends = extends.iter().map(|e| e.to_string()).collect();
        s.cell()
    }

    fn hit(file: &str, line: u32, response: Option<Shape>, request: Option<Shape>) -> Cell {
        let core = format!(
            r#"{{"method":"GET","path":"/x","file":"{file}","line":{line},"col":1,"confidence":"medium"}}"#
        );
        Cell {
            kind: cell_type::ENDPOINT_HIT,
            payload: CellPayload::Json(append_hit_fields(
                &core,
                response.as_ref(),
                request.as_ref(),
            )),
        }
    }

    fn site(form: WireForm, line: u32, status: Option<u16>, shape: Shape) -> Cell {
        let mut w = ServerWire::new(form, "v", line, shape);
        w.status = status;
        w.cell()
    }

    fn modeled(ty: &str, model: &str) -> Shape {
        Shape::Named {
            ty: ty.to_string(),
            array: false,
            model: Some(model.to_string()),
        }
    }

    fn http_edge(from: NodeId, to: NodeId, emitter: &str, rule: &str) -> Edge {
        Edge::new(from, to, edge_category::HTTP_CALLS, Confidence::Strong)
            .with_cell(Evidence::emitter(emitter).rule(rule).to_cell())
    }

    fn qname_in(m: &MergedGraph, id: NodeId) -> String {
        m.graphs
            .iter()
            .find_map(|g| g.nav.qname_by_id.get(&id).cloned())
            .unwrap_or_default()
    }

    /// web: a.ts declares Local, imports Order / ns (m.ts) and a shadowed
    /// Local; o.ts declares Stray, imported by nobody.
    fn client_graph() -> (G, NodeId) {
        let mut w = G::new("web");
        let a = w.module("a", "a.ts");
        let m = w.module("m", "m.ts");
        let o = w.module("o", "o.ts");
        let local = w.node(
            node_kind::INTERFACE,
            "a::Local",
            Some(a),
            vec![schema(WireFormat::Ts, &[("x", "string", true)], &[])],
        );
        let order = w.node(
            node_kind::INTERFACE,
            "m::Order",
            Some(m),
            vec![schema(
                WireFormat::Ts,
                &[("id", "string", true), ("items", "Item[]", true)],
                &[],
            )],
        );
        let item = w.node(
            node_kind::INTERFACE,
            "m::Item",
            Some(m),
            vec![schema(WireFormat::Ts, &[("sku", "string", true)], &[])],
        );
        let other_local = w.node(
            node_kind::INTERFACE,
            "m::Local",
            Some(m),
            vec![schema(WireFormat::Ts, &[("y", "string", true)], &[])],
        );
        let store = w.node(node_kind::CLASS, "m::Store", Some(m), vec![]);
        let goish = w.node(
            node_kind::STRUCT,
            "m::Row",
            Some(m),
            vec![schema(WireFormat::Go, &[("id", "string", true)], &[])],
        );
        let stray = w.node(
            node_kind::INTERFACE,
            "o::Stray",
            Some(o),
            vec![schema(WireFormat::Ts, &[("z", "string", true)], &[])],
        );
        w.symbol(a, "Local", local);
        w.import(a, "Local", other_local);
        w.import(a, "Order", order);
        w.import(a, "ns", m);
        w.import(a, "Store", store);
        w.import(a, "Row", goish);
        for (n, id) in [
            ("Order", order),
            ("Item", item),
            ("Local", other_local),
            ("Store", store),
            ("Row", goish),
        ] {
            w.symbol(m, n, id);
        }
        w.symbol(o, "Stray", stray);
        (w, a)
    }

    #[test]
    fn client_names_bind_own_then_import_then_namespace_never_by_name_alone() {
        let (w, a) = client_graph();
        let merged = MergedGraph::new(vec![w.g]);
        let ctx = Ctx::new(&merged);
        let q = |name: &str| {
            ctx.resolve_client_type(0, a, name)
                .map(|(_, id)| qname_in(&merged, id))
        };
        assert_eq!(
            q("Local").as_deref(),
            Some("a::Local"),
            "the module's own symbol wins over an import"
        );
        assert_eq!(q("Order").as_deref(), Some("m::Order"), "an import binding");
        assert_eq!(
            q("Order<string>").as_deref(),
            Some("m::Order"),
            "type arguments dropped"
        );
        assert_eq!(
            q("ns.Item").as_deref(),
            Some("m::Item"),
            "a namespace import's module symbols"
        );
        assert_eq!(
            q("Stray"),
            None,
            "declared elsewhere, never imported: no name-only fallback"
        );
        assert_eq!(
            q("Store"),
            None,
            "a CLASS without a schema is no client model"
        );
        assert_eq!(q("Row"), None, "a go-format STRUCT is no client model");
        assert_eq!(q("ns.Stray"), None);
        assert_eq!(
            bind_client_name(&merged.graphs[0], a, "Store")
                .map(|id| qname_in(&merged, id))
                .as_deref(),
            Some("m::Store"),
            "binding itself is any kind"
        );
    }

    /// api: Wide embeds Base (listed in extends), a tagged Inner (a field,
    /// not extends), an interface Codec and an external gorm.Model.
    fn server_graph() -> (G, BTreeMap<&'static str, NodeId>) {
        let mut s = G::new("api");
        let m = s.module("models::m", "models/m.go");
        let mut ids = BTreeMap::new();
        let base = s.node(
            node_kind::STRUCT,
            "models::m::Base",
            Some(m),
            vec![schema(
                WireFormat::Go,
                &[("id", "string", true), ("title", "int", true)],
                &[],
            )],
        );
        let inner = s.node(
            node_kind::STRUCT,
            "models::m::Inner",
            Some(m),
            vec![schema(WireFormat::Go, &[("secret", "string", true)], &[])],
        );
        let wide = s.node(
            node_kind::STRUCT,
            "models::m::Wide",
            Some(m),
            vec![schema(
                WireFormat::Go,
                &[("title", "string", true), ("inner", "Inner", true)],
                &["Base"],
            )],
        );
        let codec = s.node(node_kind::INTERFACE, "models::m::Codec", Some(m), vec![]);
        let iface = s.node(
            node_kind::STRUCT,
            "models::m::WithCodec",
            Some(m),
            vec![schema(WireFormat::Go, &[("n", "int", true)], &["Codec"])],
        );
        let ext = s.node(
            node_kind::STRUCT,
            "models::m::Row",
            Some(m),
            vec![schema(
                WireFormat::Go,
                &[("n", "int", true)],
                &["gorm.Model"],
            )],
        );
        s.edge(wide, base, edge_category::INHERITS_FROM);
        s.edge(wide, inner, edge_category::INHERITS_FROM);
        s.edge(iface, codec, edge_category::INHERITS_FROM);
        let geo = s.node(
            node_kind::STRUCT,
            "models::m::Geo",
            Some(m),
            vec![schema(WireFormat::Go, &[("lat", "float64", true)], &[])],
        );
        s.method(geo, "MarshalJSON");
        let decoded = s.node(
            node_kind::STRUCT,
            "models::m::Decoded",
            Some(m),
            vec![schema(WireFormat::Go, &[("v", "int", true)], &[])],
        );
        s.method(decoded, "UnmarshalJSON");
        let embeds_decoded = s.node(
            node_kind::STRUCT,
            "models::m::Outer",
            Some(m),
            vec![schema(WireFormat::Go, &[("w", "int", true)], &["*Decoded"])],
        );
        s.edge(embeds_decoded, decoded, edge_category::INHERITS_FROM);
        for (k, v) in [
            ("base", base),
            ("inner", inner),
            ("wide", wide),
            ("iface", iface),
            ("ext", ext),
            ("geo", geo),
            ("decoded", decoded),
            ("outer", embeds_decoded),
        ] {
            ids.insert(k, v);
        }
        (s, ids)
    }

    #[test]
    fn models_flatten_only_the_bases_extends_names() {
        let (s, ids) = server_graph();
        let merged = MergedGraph::new(vec![s.g]);
        let ctx = Ctx::new(&merged);
        let wide = ctx.model_fields(0, ids["wide"], WireDirection::Response, 0);
        assert_eq!(
            wide.fields.keys().map(String::as_str).collect::<Vec<_>>(),
            ["id", "inner", "title"]
        );
        assert!(!wide.open);
        assert_eq!(
            wide.fields["id"].declared_in,
            Some(ids["base"]),
            "a flattened base's field"
        );
        assert_eq!(
            wide.fields["title"].declared_in,
            Some(ids["wide"]),
            "the model's own name shadows the base's"
        );
        assert_eq!(wide.fields["title"].declared.as_deref(), Some("string"));
        assert!(
            !wide.fields.contains_key("secret"),
            "a tagged embed draws INHERITS_FROM but is not in extends"
        );

        let iface = ctx.model_fields(0, ids["iface"], WireDirection::Response, 0);
        assert_eq!(
            iface.fields.keys().map(String::as_str).collect::<Vec<_>>(),
            ["Codec", "n"],
            "an embedded interface is a field"
        );
        assert!(iface.fields["Codec"].value.open && !iface.open);

        let ext = ctx.model_fields(0, ids["ext"], WireDirection::Response, 0);
        assert!(
            ext.open,
            "an extends base with no INHERITS_FROM target (external) opens the model"
        );
    }

    #[test]
    fn custom_codecs_open_a_model_per_direction() {
        let (s, ids) = server_graph();
        let merged = MergedGraph::new(vec![s.g]);
        let ctx = Ctx::new(&merged);
        assert!(
            ctx.model_fields(0, ids["geo"], WireDirection::Response, 0)
                .open,
            "MarshalJSON writes what it likes"
        );
        assert!(
            !ctx.model_fields(0, ids["geo"], WireDirection::Request, 0)
                .open,
            "MarshalJSON does not decode"
        );
        assert!(
            !ctx.model_fields(0, ids["decoded"], WireDirection::Response, 0)
                .open
        );
        assert!(
            ctx.model_fields(0, ids["decoded"], WireDirection::Request, 0)
                .open
        );
        let outer = ctx.model_fields(0, ids["outer"], WireDirection::Request, 0);
        assert!(
            outer.open,
            "a flattened base's UnmarshalJSON opens the embedder"
        );
        assert_eq!(
            outer.fields.keys().map(String::as_str).collect::<Vec<_>>(),
            ["v", "w"]
        );
        assert!(
            !ctx.model_fields(0, ids["outer"], WireDirection::Response, 0)
                .open
        );
    }

    /// One web graph calling three endpoints and one api graph serving them.
    fn exchange() -> (G, G) {
        let (mut w, a) = client_graph();
        let order = Shape::named("Order");
        let e1 = w.node(
            node_kind::ENDPOINT,
            "endpoint:GET:/orders",
            None,
            vec![hit("a.ts", 7, Some(order.clone()), None)],
        );
        let e2 = w.node(
            node_kind::ENDPOINT,
            "endpoint:POST:/orders @web",
            None,
            vec![
                hit(
                    "a.ts",
                    9,
                    Some(Shape::Union(vec![order.clone(), Shape::named("Nope")])),
                    Some(Shape::Object {
                        fields: vec![
                            ShapeField::new("item", true, Shape::named("ns.Item")),
                            ShapeField::new("tag", true, Shape::named("Alias")),
                        ],
                        open: false,
                    }),
                ),
                hit("a.ts", 3, Some(Shape::open("res.body")), None),
            ],
        );
        let e3 = w.node(
            node_kind::ENDPOINT,
            "endpoint:GET:/other",
            None,
            vec![hit("a.ts", 12, Some(order), None)],
        );
        let _ = (a, e1, e2, e3);

        let (mut s, ids) = server_graph();
        let h = s.module("handlers::h", "handlers/h.go");
        let get = s.node(
            node_kind::FUNCTION,
            "handlers::h::Get",
            Some(h),
            vec![
                pos("handlers/h.go"),
                site(WireForm::JsonResponse, 20, Some(404), Shape::named("gin.H")),
                site(WireForm::JsonResponse, 21, None, Shape::open("err")),
                site(
                    WireForm::JsonResponse,
                    22,
                    Some(200),
                    modeled("models.Wide", "models::m::Wide"),
                ),
            ],
        );
        let post = s.node(
            node_kind::FUNCTION,
            "handlers::h::Post",
            Some(h),
            vec![
                pos("handlers/h.go"),
                site(
                    WireForm::JsonBind,
                    30,
                    None,
                    modeled("models.Base", "models::m::Base"),
                ),
                site(
                    WireForm::JsonResponse,
                    33,
                    Some(201),
                    Shape::Object {
                        fields: vec![
                            ShapeField::new("geo", true, modeled("models.Geo", "models::m::Geo")),
                            ShapeField::new(
                                "n",
                                true,
                                Shape::Call {
                                    chain: "strconv.Atoi()".into(),
                                    array: false,
                                    model: None,
                                },
                            ),
                        ],
                        open: false,
                    },
                ),
            ],
        );
        let r1 = s.node(node_kind::ROUTE, "GET /orders", None, vec![]);
        let r2 = s.node(node_kind::ROUTE, "POST /orders", None, vec![]);
        let r3 = s.node(node_kind::ROUTE, "GET /other", None, vec![]);
        s.edge(r1, get, edge_category::HANDLED_BY);
        s.edge(r2, post, edge_category::HANDLED_BY);
        s.edge(r3, get, edge_category::HANDLED_BY);
        let _ = ids;
        (w, s)
    }

    fn merged_of(w: G, s: G, web_first: bool) -> MergedGraph {
        let id = |g: &G, q: &str| {
            g.g.nodes
                .iter()
                .find(|n| g.g.nav.qname_by_id[&n.id] == q)
                .map(|n| n.id)
                .unwrap_or(NodeId(0))
        };
        let mut cross = vec![
            http_edge(
                id(&w, "endpoint:GET:/orders"),
                id(&s, "GET /orders"),
                HTTP_EMITTER,
                "exact",
            ),
            http_edge(
                id(&w, "endpoint:POST:/orders @web"),
                id(&s, "POST /orders"),
                HTTP_EMITTER,
                "base_fold",
            ),
            http_edge(
                id(&w, "endpoint:GET:/other"),
                id(&s, "GET /other"),
                HTTP_EMITTER,
                WEAK_RULE,
            ),
            http_edge(
                id(&w, "endpoint:GET:/other"),
                id(&s, "GET /orders"),
                "overlay",
                "declared",
            ),
        ];
        let graphs = if web_first {
            vec![w.g, s.g]
        } else {
            cross.reverse();
            vec![s.g, w.g]
        };
        let mut m = MergedGraph::new(graphs);
        m.cross_edges = cross;
        m
    }

    #[test]
    fn client_wires_pair_resolved_sides_through_the_http_pairing() {
        let (w, s) = exchange();
        let merged = merged_of(w, s, true);
        let cw = client_wires(&merged);
        assert_eq!(
            cw.marker(),
            "[models] client_wires hits=4 routed=3 response=4 request=1 client_models=2 unresolved_client=1 \
             server_sites=3 server_unknown_status=1 open=2 weak_pairing=1"
        );
        let order: Vec<(&str, &str, u32)> = cw
            .wires
            .iter()
            .map(|w| (w.endpoint_qname.as_str(), w.file.as_str(), w.line))
            .collect();
        assert_eq!(
            order,
            [
                ("endpoint:GET:/orders", "a.ts", 7),
                ("endpoint:POST:/orders @web", "a.ts", 3),
                ("endpoint:POST:/orders @web", "a.ts", 9),
                ("endpoint:GET:/other", "a.ts", 12),
            ],
            "routed first by route qname, then (file, line); the suffix-only and overlay pairs leave GET /other unrouted"
        );
        let get = &cw.wires[0];
        assert_eq!(
            qname_in(
                &merged,
                get.response
                    .as_ref()
                    .and_then(|r| r.model)
                    .unwrap_or(NodeId(0))
            ),
            "m::Order"
        );
        let items = &get.response.as_ref().unwrap().fields["items"].value;
        assert!(
            items.array && items.model.is_some(),
            "a TS field's element binds in its model's own module"
        );
        assert_eq!(get.routes.len(), 1);
        assert_eq!(get.routes[0].rule, "exact");
        let handler = &get.routes[0].handlers[0];
        assert_eq!(handler.qname, "handlers::h::Get");
        assert_eq!(
            handler.response.len(),
            1,
            "the 404 and the unknown-status sites are never sites"
        );
        assert_eq!(
            (handler.response[0].line, handler.response[0].status),
            (22, Some(200))
        );
        let server = &handler.response[0].shape;
        assert_eq!(
            qname_in(&merged, server.model.unwrap_or(NodeId(0))),
            "models::m::Wide"
        );
        assert_eq!(
            server.fields.keys().map(String::as_str).collect::<Vec<_>>(),
            ["id", "inner", "title"]
        );
        assert_eq!(server.format, Some(WireFormat::Go));

        let post = &cw.wires[2];
        assert_eq!(post.owner.as_deref(), Some("web"));
        let req = post.request.as_ref().unwrap();
        assert_eq!(
            qname_in(&merged, req.fields["item"].value.model.unwrap_or(NodeId(0))),
            "m::Item",
            "an inline key's namespace-qualified name"
        );
        assert!(
            req.fields["tag"].value.open && req.fields["tag"].value.model.is_none(),
            "an inline key that binds nothing stays open, uncounted"
        );
        let resp = post.response.as_ref().unwrap();
        assert_eq!(resp.members.len(), 2);
        assert!(
            resp.members[1].open,
            "Nope binds nothing: the root counts unresolved"
        );
        let side = &post.routes[0];
        assert_eq!(side.rule, "base_fold");
        let bind = &side.handlers[0].request;
        assert_eq!(bind.len(), 1, "a bind carries no status and is a site");
        assert_eq!(
            qname_in(&merged, bind[0].shape.model.unwrap_or(NodeId(0))),
            "models::m::Base"
        );
        let env = &side.handlers[0].response[0].shape;
        assert!(
            env.fields["geo"].value.open,
            "Geo's MarshalJSON opens it in a response"
        );
        assert!(env.fields["n"].value.open, "an untyped call is open");
        assert!(!env.open, "the envelope itself lists its keys");
        assert!(cw.wires[1].response.as_ref().unwrap().open);
        assert!(cw.wires[3].routes.is_empty());
    }

    #[test]
    fn client_wires_are_deterministic_across_graph_order() {
        let (w, s) = exchange();
        let a = client_wires(&merged_of(w, s, true));
        let (w, s) = exchange();
        let b = client_wires(&merged_of(w, s, false));
        assert_eq!(a, b);
        let (w, s) = exchange();
        assert_eq!(a, client_wires(&merged_of(w, s, true)));
        assert_eq!(
            client_wires(&MergedGraph::new(vec![])),
            ClientWires::default()
        );
    }

    #[test]
    fn a_void_response_names_no_model() {
        let (mut w, _) = client_graph();
        w.node(
            node_kind::ENDPOINT,
            "endpoint:DELETE:/orders",
            None,
            vec![hit("a.ts", 5, Some(Shape::named("void")), None)],
        );
        w.node(
            node_kind::ENDPOINT,
            "endpoint:GET:/translations",
            None,
            vec![hit("a.ts", 6, Some(Shape::named("Translation")), None)],
        );
        let cw = client_wires(&MergedGraph::new(vec![w.g]));
        assert_eq!(
            (cw.stats.hits, cw.stats.unresolved_client, cw.stats.open),
            (2, 1, 2),
            "void is no model; an external name is unresolved"
        );
    }

    #[test]
    fn depth_is_capped_and_cycles_terminate() {
        let mut s = G::new("deep");
        let m = s.module("models::d", "models/d.go");
        let mut node_field = ModelField::new("next", "*Node", false);
        node_field.model = Some("models::d::Node".into());
        let mut sch = ModelSchema::new(WireFormat::Go);
        sch.fields = vec![node_field];
        sch.extends = vec!["Node".into()];
        let n = s.node(
            node_kind::STRUCT,
            "models::d::Node",
            Some(m),
            vec![sch.cell()],
        );
        s.edge(n, n, edge_category::INHERITS_FROM);
        let merged = MergedGraph::new(vec![s.g]);
        let ctx = Ctx::new(&merged);
        let mut f = ctx.model_fields(0, n, WireDirection::Response, 0);
        assert!(
            f.open,
            "a base already on the extends chain opens rather than loops"
        );
        let mut levels = 0;
        while let Some(next) = f.fields.get("next").map(|x| x.value.clone()) {
            levels += 1;
            if next.fields.is_empty() {
                assert!(
                    next.open && next.model == Some(n),
                    "past MAX_DEPTH the model is listed, open, unexpanded"
                );
                break;
            }
            f = ResolvedFields {
                fields: next.fields,
                open: next.open,
            };
        }
        assert_eq!(levels, MAX_DEPTH);
    }

    // ---- DT.1m: wire pairs ----------------------------------------------------

    /// A value declared `ty` in `format`, resolved to no model.
    fn val(format: WireFormat, ty: &str) -> ResolvedShape {
        let (elem, array) = element_type(ty);
        let kind = json_kind(format, &elem);
        ResolvedShape {
            written: Shape::Named {
                ty: elem,
                array,
                model: None,
            },
            format: Some(format),
            model: None,
            array,
            fields: BTreeMap::new(),
            members: Vec::new(),
            open: matches!(
                kind,
                JsonKind::Unknown(_) | JsonKind::Any | JsonKind::Object
            ),
        }
    }

    /// Model `id` (an array of it when `array`) with `fields`.
    fn model_of(
        format: WireFormat,
        id: u64,
        array: bool,
        open: bool,
        fields: Vec<ResolvedField>,
    ) -> ResolvedShape {
        ResolvedShape {
            written: Shape::Named {
                ty: format!("M{id}"),
                array,
                model: None,
            },
            format: Some(format),
            model: Some(NodeId(id)),
            array,
            fields: fields.into_iter().map(|f| (f.name.clone(), f)).collect(),
            members: Vec::new(),
            open,
        }
    }

    /// An inline object with `fields`.
    fn inline(format: WireFormat, open: bool, fields: Vec<ResolvedField>) -> ResolvedShape {
        ResolvedShape {
            written: Shape::Object {
                fields: Vec::new(),
                open,
            },
            format: Some(format),
            model: None,
            array: false,
            fields: fields.into_iter().map(|f| (f.name.clone(), f)).collect(),
            members: Vec::new(),
            open,
        }
    }

    fn union_of(format: WireFormat, members: Vec<ResolvedShape>) -> ResolvedShape {
        ResolvedShape {
            written: Shape::Union(members.iter().map(|m| m.written.clone()).collect()),
            format: Some(format),
            model: None,
            array: false,
            fields: BTreeMap::new(),
            open: members.iter().any(|m| m.open),
            members,
        }
    }

    fn fld(
        name: &str,
        declared: Option<&str>,
        required: bool,
        value: ResolvedShape,
    ) -> ResolvedField {
        ResolvedField {
            name: name.to_string(),
            member: None,
            declared: declared.map(str::to_string),
            required,
            bind_required: false,
            declared_in: None,
            value,
        }
    }

    /// A model field declared `ty`, its value read off `ty`.
    fn mf(format: WireFormat, name: &str, ty: &str, required: bool) -> ResolvedField {
        fld(name, Some(ty), required, val(format, ty))
    }

    /// `f` with the binder requiring it.
    fn bound(mut f: ResolvedField) -> ResolvedField {
        f.bind_required = true;
        f
    }

    fn changes(c: &Compared) -> Vec<(&str, &'static str, bool)> {
        c.changes
            .iter()
            .map(|x| (x.path.as_str(), x.rule, x.breaking))
            .collect()
    }

    const TS: WireFormat = WireFormat::Ts;
    const GO: WireFormat = WireFormat::Go;

    #[test]
    fn response_rules_name_every_change() {
        let item = |format: WireFormat, sku: &str, qty: &str| {
            vec![mf(format, "sku", sku, true), mf(format, "qty", qty, true)]
        };
        let client = model_of(
            TS,
            1,
            false,
            false,
            vec![
                mf(TS, "a", "string", true),
                mf(TS, "b", "string", false),
                mf(TS, "c", "string", true),
                mf(TS, "n", "number", true),
                mf(TS, "tags", "string[]", true),
                mf(TS, "when", "Date", true),
                fld(
                    "items",
                    Some("Item[]"),
                    true,
                    model_of(TS, 2, true, false, item(TS, "string", "number")),
                ),
            ],
        );
        let server = model_of(
            GO,
            11,
            false,
            false,
            vec![
                mf(GO, "a", "string", false),
                mf(GO, "d", "string", true),
                mf(GO, "n", "string", true),
                mf(GO, "tags", "string", true),
                mf(GO, "when", "int64", true),
                fld(
                    "items",
                    Some("[]Item"),
                    true,
                    model_of(GO, 12, true, false, item(GO, "int", "int")),
                ),
            ],
        );
        let cmp = compare(WireDirection::Response, &client, &server, "");
        assert_eq!(
            changes(&cmp),
            [
                ("a", FIELD_MAY_BE_ABSENT, false),
                ("b", FIELD_OPTIONAL_ABSENT, false),
                ("c", FIELD_MISSING, true),
                ("d", FIELD_UNREAD, false),
                ("items[].sku", TYPE_MISMATCH, true),
                ("n", TYPE_MISMATCH, true),
                ("tags", TYPE_MISMATCH, true),
            ],
            "Date is any: never judged"
        );
        let tags = cmp.changes.iter().find(|c| c.path == "tags").unwrap();
        assert_eq!(
            (tags.client.as_deref(), tags.server.as_deref()),
            (Some("string[]"), Some("string")),
            "an array against a scalar"
        );
        assert!(cmp.notes.is_empty());
        assert_eq!(
            cmp.models
                .iter()
                .map(|m| (m.client.0, m.server.0, m.path.as_str(), m.diverged))
                .collect::<Vec<_>>(),
            [(1, 11, "", true), (2, 12, "items[]", true)]
        );
    }

    #[test]
    fn request_rules_fold_names_into_a_go_binder_only() {
        let client = model_of(
            TS,
            1,
            false,
            false,
            vec![
                mf(TS, "customername", "string", true),
                mf(TS, "extra", "string", false),
                mf(TS, "id", "string", true),
            ],
        );
        let server = |format: WireFormat| {
            model_of(
                format,
                11,
                false,
                false,
                vec![
                    bound(mf(format, "customerName", "string", true)),
                    mf(format, "id", "string", true),
                    bound(mf(format, "note", "string", true)),
                    mf(format, "memo", "string", true),
                ],
            )
        };
        let into_go = compare(WireDirection::Request, &client, &server(GO), "");
        assert_eq!(
            changes(&into_go),
            [
                ("extra", FIELD_DROPPED, false),
                ("memo", FIELD_UNSET, false),
                ("note", REQUIRED_MISSING, true),
            ],
            "encoding/json folds customername onto customerName"
        );
        let into_ts = compare(WireDirection::Request, &client, &server(TS), "");
        assert_eq!(
            changes(&into_ts),
            [
                ("customerName", REQUIRED_MISSING, true),
                ("customername", FIELD_DROPPED, false),
                ("extra", FIELD_DROPPED, false),
                ("memo", FIELD_UNSET, false),
                ("note", REQUIRED_MISSING, true),
            ],
            "no folding into a non-Go server"
        );
        let response = compare(WireDirection::Response, &client, &server(GO), "");
        assert!(
            changes(&response).contains(&("customername", FIELD_MISSING, true)),
            "a response never folds: the client reads exact names"
        );
        assert_eq!(
            match_names(["Name", "id"], ["ID", "id", "name"], true),
            [("Name", "name"), ("id", "id")],
            "an exact match first; ID stays free"
        );
        assert_eq!(match_names(["ID"], ["Id", "id"], true), [("ID", "Id")]);
        assert!(match_names(["ID"], ["Id", "id"], false).is_empty());
    }

    #[test]
    fn open_sides_are_never_claimed_to_lack_a_name() {
        let closed = |id: u64, names: &[&str]| {
            model_of(
                TS,
                id,
                false,
                false,
                names.iter().map(|n| mf(TS, n, "string", true)).collect(),
            )
        };
        let open_server = model_of(
            GO,
            11,
            false,
            true,
            vec![mf(GO, "a", "string", true), mf(GO, "c", "string", true)],
        );
        let cmp = compare(
            WireDirection::Response,
            &closed(1, &["a", "b"]),
            &open_server,
            "",
        );
        assert_eq!(
            changes(&cmp),
            [("c", FIELD_UNREAD, false)],
            "b may be written by the open server; c is unread by the closed client"
        );
        assert_eq!(cmp.notes.iter().copied().collect::<Vec<_>>(), [NOTE_OPEN]);

        let open_client = union_of(TS, vec![closed(1, &["a", "b"]), val(TS, "any")]);
        let server = model_of(
            GO,
            11,
            false,
            false,
            vec![mf(GO, "a", "string", true), mf(GO, "c", "string", true)],
        );
        let cmp = compare(WireDirection::Response, &open_client, &server, "");
        assert!(cmp.changes.is_empty(), "{:?}", cmp.changes);
        assert_eq!(
            cmp.notes.iter().copied().collect::<Vec<_>>(),
            [NOTE_OPEN, NOTE_UNION],
            "a union with an open member is open"
        );

        let body = |v: ResolvedShape| inline(GO, false, vec![fld("x", None, true, v)]);
        let cmp = compare(
            WireDirection::Response,
            &inline(
                TS,
                false,
                vec![fld("x", None, true, closed(2, &["p", "q"]))],
            ),
            &body(ResolvedShape {
                written: Shape::Call {
                    chain: "h.load()".into(),
                    array: false,
                    model: None,
                },
                format: Some(GO),
                model: None,
                array: false,
                fields: BTreeMap::new(),
                members: Vec::new(),
                open: true,
            }),
            "",
        );
        assert!(cmp.changes.is_empty());
        assert!(
            cmp.notes.contains(NOTE_OPEN),
            "a model against an untyped call"
        );
    }

    #[test]
    fn unions_of_objects_compare_their_merged_names() {
        let a = model_of(
            TS,
            1,
            false,
            false,
            vec![mf(TS, "id", "string", true), mf(TS, "x", "string", true)],
        );
        let b = model_of(
            TS,
            2,
            false,
            false,
            vec![mf(TS, "id", "string", true), mf(TS, "y", "string", true)],
        );
        let server = model_of(
            GO,
            11,
            false,
            false,
            vec![mf(GO, "id", "string", true), mf(GO, "z", "string", true)],
        );
        let cmp = compare(
            WireDirection::Response,
            &union_of(TS, vec![a, b]),
            &server,
            "",
        );
        assert_eq!(
            changes(&cmp),
            [
                ("x", FIELD_OPTIONAL_ABSENT, false),
                ("y", FIELD_OPTIONAL_ABSENT, false),
                ("z", FIELD_UNREAD, false),
            ],
            "a name one member lacks is optional"
        );
        assert_eq!(cmp.notes.iter().copied().collect::<Vec<_>>(), [NOTE_UNION]);
        assert!(cmp.models.is_empty(), "a union is no model");
    }

    #[test]
    fn unknown_names_are_judged_only_as_models_and_tuples_only_as_arrays() {
        let user = model_of(TS, 2, false, false, vec![mf(TS, "name", "string", true)]);
        let client = inline(
            TS,
            false,
            vec![
                mf(TS, "status", "InvoiceStatus", true),
                fld("owner", Some("User"), true, user),
                mf(TS, "anything", "any", true),
                mf(TS, "pair", "[string, number]", true),
                mf(TS, "other", "[string, number]", true),
                fld("page", None, true, val(TS, "number")),
                mf(TS, "nothing", "null", true),
            ],
        );
        let call = ResolvedShape {
            written: Shape::Call {
                chain: "strconv.Atoi()".into(),
                array: false,
                model: None,
            },
            format: Some(GO),
            model: None,
            array: false,
            fields: BTreeMap::new(),
            members: Vec::new(),
            open: true,
        };
        let server = inline(
            GO,
            false,
            vec![
                mf(GO, "status", "string", true),
                mf(GO, "owner", "string", true),
                mf(GO, "anything", "int", true),
                mf(GO, "pair", "[]int", true),
                mf(GO, "other", "string", true),
                fld("page", None, true, call),
                mf(GO, "nothing", "string", true),
            ],
        );
        let cmp = compare(WireDirection::Response, &client, &server, "");
        assert_eq!(
            changes(&cmp),
            [
                ("other", TYPE_MISMATCH, true),
                ("owner", TYPE_MISMATCH, true)
            ],
            "an unbound alias, any, null and an untyped call are never judged; a tuple's elements neither"
        );
        assert!(
            cmp.notes.is_empty(),
            "no open side was compared field by field"
        );
    }

    #[test]
    fn confirms_needs_one_shared_name_and_half_the_smaller_side() {
        assert_eq!(
            confirms(["a"], ["a", "b", "c"], false),
            Some(1),
            "a 1-field side"
        );
        assert_eq!(confirms(["a"], ["b"], false), None);
        assert_eq!(
            confirms([] as [&str; 0], ["a"], false),
            None,
            "an empty side"
        );
        assert_eq!(
            confirms(["a", "b", "c", "d"], ["a", "b", "x", "y"], false),
            Some(2),
            "half"
        );
        assert_eq!(
            confirms(["a", "b", "c", "d"], ["a", "x", "y", "z"], false),
            None,
            "under half"
        );
        assert_eq!(confirms(["Id", "name"], ["id", "x"], false), None);
        assert_eq!(
            confirms(["Id", "name"], ["id", "x"], true),
            Some(1),
            "folded"
        );
    }

    #[test]
    fn model_pairs_recurse_into_same_named_models_under_inline_roots() {
        let offer = |format: WireFormat, id: u64, extra: &str| {
            model_of(
                format,
                id,
                true,
                false,
                vec![
                    mf(format, "id", "string", true),
                    mf(format, "price", "number", true),
                    mf(format, extra, "string", true),
                ],
            )
        };
        let account = |format: WireFormat, id: u64, names: &[&str]| {
            model_of(
                format,
                id,
                false,
                false,
                names
                    .iter()
                    .map(|n| mf(format, n, "string", true))
                    .collect(),
            )
        };
        let mut go_offer = offer(GO, 12, "seller");
        go_offer
            .fields
            .insert("price".into(), mf(GO, "price", "int", true));
        let client = inline(
            TS,
            false,
            vec![
                fld("offers", None, true, offer(TS, 2, "seller")),
                fld("owner", None, true, account(TS, 3, &["a", "b", "c", "d"])),
            ],
        );
        let server = inline(
            GO,
            false,
            vec![
                fld("offers", None, true, go_offer),
                fld("owner", None, true, account(GO, 13, &["a", "x", "y", "z"])),
            ],
        );
        let cmp = compare(WireDirection::Response, &client, &server, "");
        assert_eq!(
            cmp.models
                .iter()
                .map(|m| (
                    m.client.0,
                    m.server.0,
                    m.path.as_str(),
                    m.overlap,
                    m.diverged
                ))
                .collect::<Vec<_>>(),
            [(2, 12, "offers[]", 3, false)],
            "owner shares one name of four: no pair"
        );
        assert_eq!(
            changes(&cmp),
            [
                ("owner.b", FIELD_MISSING, true),
                ("owner.c", FIELD_MISSING, true),
                ("owner.d", FIELD_MISSING, true),
                ("owner.x", FIELD_UNREAD, false),
                ("owner.y", FIELD_UNREAD, false),
                ("owner.z", FIELD_UNREAD, false),
            ],
            "an unconfirmed pair's drift is still drift"
        );

        // A nested change diverges the nested pair, not its parent.
        let item = |format: WireFormat, id: u64, ty: &str| {
            model_of(format, id, true, false, vec![mf(format, "sku", ty, true)])
        };
        let order = |format: WireFormat, id: u64, items: ResolvedShape, decl: &str| {
            model_of(
                format,
                id,
                false,
                false,
                vec![
                    mf(format, "id", "string", true),
                    fld("items", Some(decl), true, items),
                ],
            )
        };
        let cmp = compare(
            WireDirection::Response,
            &order(TS, 1, item(TS, 2, "string"), "Item[]"),
            &order(GO, 11, item(GO, 12, "int"), "[]Item"),
            "",
        );
        assert_eq!(changes(&cmp), [("items[].sku", TYPE_MISMATCH, true)]);
        assert_eq!(
            cmp.models
                .iter()
                .map(|m| (m.path.as_str(), m.diverged))
                .collect::<Vec<_>>(),
            [("", false), ("items[]", true)]
        );
    }

    fn server_site(line: u32, form: WireForm, shape: ResolvedShape) -> ServerSite {
        ServerSite {
            handler: NodeId(99),
            form,
            line,
            status: (form == WireForm::JsonResponse).then_some(200),
            shape,
        }
    }

    #[test]
    fn the_server_side_is_the_one_distinct_shape_or_the_best_overlap() {
        let names = |format: WireFormat, id: u64, names: &[&str]| {
            model_of(
                format,
                id,
                false,
                false,
                names
                    .iter()
                    .map(|n| mf(format, n, "string", true))
                    .collect(),
            )
        };
        let client = names(TS, 1, &["id", "name", "email"]);
        let user = server_site(
            10,
            WireForm::JsonResponse,
            names(GO, 11, &["id", "name", "email"]),
        );
        let again = server_site(
            14,
            WireForm::JsonResponse,
            names(GO, 11, &["id", "name", "email"]),
        );
        let token = server_site(
            18,
            WireForm::JsonResponse,
            inline(
                GO,
                true,
                vec![fld("tokenSync", None, true, val(GO, "bool"))],
            ),
        );
        let resp = WireDirection::Response;
        let pick = |sites: &[ServerSite]| pick_server(resp, &client, sites).map(|s| s.line);
        assert_eq!(
            pick(&[user.clone(), again.clone()]),
            Some(10),
            "one model twice is one shape"
        );
        assert_eq!(
            pick(&[token.clone(), user.clone(), again.clone()]),
            Some(10),
            "the largest top-level overlap wins"
        );
        let other = server_site(
            22,
            WireForm::JsonResponse,
            names(GO, 12, &["id", "name", "email"]),
        );
        assert_eq!(
            pick(&[user.clone(), other.clone()]),
            None,
            "a tie: ambiguous"
        );
        let bind =
            |line: u32, id: u64| server_site(line, WireForm::JsonBind, names(GO, id, &["id"]));
        let req = WireDirection::Request;
        assert_eq!(
            pick_server(req, &client, &[bind(5, 11), bind(9, 11)]).map(|s| s.line),
            Some(5)
        );
        assert_eq!(
            pick_server(req, &client, &[bind(5, 11), bind(9, 12)]).map(|s| s.line),
            None,
            "a request needs exactly one bind"
        );
        assert_eq!(pick_server(req, &client, &[]).map(|s| s.line), None);
    }

    #[test]
    fn wire_pairs_compare_the_exchange_and_note_what_they_cannot_judge() {
        let (w, s) = exchange();
        let merged = merged_of(w, s, true);
        let wp = wire_pairs(&merged);
        assert_eq!(
            wp.marker(),
            "[models] pairs=4 response=3 request=1 model_pairs=1 changes=7 breaking=1 \
             ambiguous_server=0 open=2"
        );
        let rows: Vec<(&str, &str, u32, Vec<(&str, &'static str, bool)>, Vec<&str>)> = wp
            .pairs
            .iter()
            .map(|p| {
                (
                    p.route_qname.as_str(),
                    p.direction.as_str(),
                    p.line,
                    p.changes
                        .iter()
                        .map(|c| (c.path.as_str(), c.rule, c.breaking))
                        .collect(),
                    p.notes.clone(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "GET /orders",
                    "response",
                    7,
                    vec![
                        ("inner", FIELD_UNREAD, false),
                        ("items", FIELD_MISSING, true),
                        ("title", FIELD_UNREAD, false),
                    ],
                    vec![],
                ),
                ("POST /orders", "response", 3, vec![], vec![NOTE_OPEN]),
                (
                    "POST /orders",
                    "response",
                    9,
                    vec![],
                    vec![NOTE_OPEN, NOTE_UNION]
                ),
                (
                    "POST /orders",
                    "request",
                    9,
                    vec![
                        ("id", FIELD_UNSET, false),
                        ("item", FIELD_DROPPED, false),
                        ("tag", FIELD_DROPPED, false),
                        ("title", FIELD_UNSET, false),
                    ],
                    vec![],
                ),
            ]
        );
        assert_eq!(
            wp.model_pairs
                .iter()
                .map(|p| (
                    p.client_qname.as_str(),
                    p.server_qname.as_str(),
                    p.overlap,
                    p.diverged
                ))
                .collect::<Vec<_>>(),
            [("m::Order", "models::m::Wide", 1, true)],
            "one of Order's two names is half: confirmed"
        );
        let get = &wp.pairs[0];
        assert_eq!(get.server_line, Some(22));
        assert_eq!(get.server_format, Some(WireFormat::Go));
        assert_eq!(get.client_format, Some(WireFormat::Ts));
    }

    #[test]
    fn an_ambiguous_server_compares_nothing() {
        let (w, mut s) = exchange();
        let get =
            s.g.nodes
                .iter_mut()
                .find(|n| s.g.nav.qname_by_id[&n.id] == "handlers::h::Get")
                .unwrap();
        get.cells.push(site(
            WireForm::JsonResponse,
            25,
            Some(200),
            modeled("models.Base", "models::m::Base"),
        ));
        let wp = wire_pairs(&merged_of(w, s, true));
        let p = &wp.pairs[0];
        assert_eq!(
            (
                p.route_qname.as_str(),
                p.notes.as_slice(),
                p.changes.len(),
                p.server_line
            ),
            ("GET /orders", [NOTE_AMBIGUOUS_SERVER].as_slice(), 0, None),
            "Wide and Base each share `id` with Order: a tie"
        );
        assert_eq!(wp.stats.ambiguous_server, 1);
        assert!(wp.model_pairs.is_empty());
    }

    #[test]
    fn wire_pairs_are_deterministic_across_graph_order() {
        let (w, s) = exchange();
        let a = wire_pairs(&merged_of(w, s, true));
        let (w, s) = exchange();
        let b = wire_pairs(&merged_of(w, s, false));
        assert_eq!(a, b);
        let (w, s) = exchange();
        assert_eq!(a, wire_pairs(&merged_of(w, s, true)));
        assert_eq!(wire_pairs(&MergedGraph::new(vec![])), WirePairs::default());
    }

    // ---- DT.1h: http_model identity ---------------------------------------

    fn join(route: &str, file: &str, line: u32, diverged: bool) -> ModelJoin {
        ModelJoin {
            direction: WireDirection::Response,
            route_qname: route.to_string(),
            file: file.to_string(),
            line,
            handler_qname: "h".to_string(),
            path: String::new(),
            overlap: 2,
            client_fields: 2,
            server_fields: 2,
            diverged,
        }
    }

    fn mpair(client: (u64, &str), server: (u64, &str), diverged: bool) -> ModelPair {
        ModelPair {
            client: NodeId(client.0),
            client_qname: client.1.to_string(),
            server: NodeId(server.0),
            server_qname: server.1.to_string(),
            overlap: 2,
            diverged,
            joins: vec![join("GET /x", "a.ts", 3, diverged)],
        }
    }

    /// `(server, [clients])` qnames of each star.
    fn star_rows(stars: &[ModelStar<'_>], pairs: &[ModelPair]) -> Vec<(String, Vec<String>)> {
        let q = |id: NodeId| {
            pairs
                .iter()
                .find_map(|p| (p.server == id).then(|| p.server_qname.clone()))
                .unwrap_or_default()
        };
        stars
            .iter()
            .map(|s| {
                (
                    q(s.server),
                    s.clients.iter().map(|p| p.client_qname.clone()).collect(),
                )
            })
            .collect()
    }

    /// The four rules of [`model_stars`]: the embedded base is kept, two
    /// unrelated servers refuse the client, two clients of one project
    /// refuse each other, clients of two projects share a star.
    #[test]
    fn model_stars_keep_the_embedded_base_and_refuse_ambiguity() {
        let pairs = vec![
            // web: Listing pairs a struct and its embedder.
            mpair((1, "web::Listing"), (101, "api::Listing"), true),
            mpair((1, "web::Listing"), (102, "api::ListingWithSeller"), false),
            // web: Summary pairs two structs neither embeds.
            mpair((2, "web::Summary"), (103, "api::Daily"), false),
            mpair((2, "web::Summary"), (104, "api::Weekly"), false),
            // web: two views of one struct in one project.
            mpair((3, "web::Order"), (105, "api::Order"), false),
            mpair((4, "web::OrderView"), (105, "api::Order"), false),
            // web and admin: one struct, one client model each.
            mpair((5, "web::Item"), (106, "api::Item"), false),
            mpair((6, "admin::Item"), (106, "api::Item"), true),
            // a kind the rule does not pair.
            mpair((7, "web::Skipped"), (107, "api::Skipped"), false),
        ];
        let embeds = |s: NodeId, other: NodeId| (s.0, other.0) == (102, 101);
        let project = |c: NodeId| -> ProjectKey {
            let p = if c.0 == 6 { "admin" } else { "web" };
            (9, Some(p.to_string()))
        };
        let takes = |p: &ModelPair| p.client.0 != 7;
        let (stars, stats) = model_stars(&pairs, takes, embeds, project);
        let rows = star_rows(&stars, &pairs);
        assert_eq!(
            rows,
            [
                (
                    "api::Item".to_string(),
                    vec!["admin::Item".to_string(), "web::Item".to_string()]
                ),
                ("api::Listing".to_string(), vec!["web::Listing".to_string()]),
            ]
        );
        assert_eq!(
            stats,
            StarStats {
                ambiguous_client: 1,
                ambiguous_project: 2,
                embed_reduced: 1,
            }
        );

        // Order free: the stars do not depend on the pairs' order.
        let mut rev = pairs.clone();
        rev.reverse();
        let (again, stats_again) = model_stars(&rev, takes, embeds, project);
        assert_eq!((star_rows(&again, &rev), stats_again), (rows, stats));

        // Mutual embeds (a cycle) leave nothing to pair.
        let cyc = |s: NodeId, o: NodeId| matches!((s.0, o.0), (102, 101) | (101, 102));
        let (stars, stats) = model_stars(&pairs[..2], |_| true, cyc, project);
        assert!(stars.is_empty());
        assert_eq!((stats.embed_reduced, stats.ambiguous_client), (1, 0));
    }

    /// The deepest enclosing PROJECT by its ORIGIN path; the root encloses
    /// every file, another path only at a `/` boundary.
    #[test]
    fn project_paths_pick_the_deepest_enclosing_project() {
        let mut p = G::new("projects");
        let origin = |path: &str| Cell {
            kind: cell_type::ORIGIN,
            payload: CellPayload::Json(format!(
                r#"{{"label":"x","path":"{path}","provenance":"project_root"}}"#
            )),
        };
        p.node(node_kind::PROJECT, "project:.", None, vec![origin("")]);
        p.node(node_kind::PROJECT, "project:web", None, vec![origin("web")]);
        p.node(
            node_kind::PROJECT,
            "project:web/admin",
            None,
            vec![origin("web/admin")],
        );
        let repo = p.g.repo;
        let m = MergedGraph::new(vec![p.g]);
        let paths = ProjectPaths::of(&m);
        assert_eq!(paths.owner_of(repo, "web/src/a.ts"), Some("web"));
        assert_eq!(
            paths.owner_of(repo, "web/admin/src/a.ts"),
            Some("web/admin")
        );
        assert_eq!(
            paths.owner_of(repo, "webx/a.ts"),
            Some(""),
            "not under `web`"
        );
        assert_eq!(paths.owner_of(repo, "api/main.go"), Some(""));
        assert_eq!(paths.owner_of(RepoId(1), "web/a.ts"), None, "another repo");
    }

    /// `exchange()` plus a second exchange: `GET /base` answers the same
    /// client `Order` with `models.Base`, the struct `Wide` embeds.
    fn exchange_with_base(web_first: bool) -> MergedGraph {
        let (mut w, mut s) = exchange();
        let e = w.node(
            node_kind::ENDPOINT,
            "endpoint:GET:/base",
            None,
            vec![hit("a.ts", 15, Some(Shape::named("Order")), None)],
        );
        let h = s.module("handlers::b", "handlers/b.go");
        let base = s.node(
            node_kind::FUNCTION,
            "handlers::b::Base",
            Some(h),
            vec![
                pos("handlers/b.go"),
                site(
                    WireForm::JsonResponse,
                    5,
                    Some(200),
                    modeled("models.Base", "models::m::Base"),
                ),
            ],
        );
        let r = s.node(node_kind::ROUTE, "GET /base", None, vec![]);
        s.edge(r, base, edge_category::HANDLED_BY);
        let mut m = merged_of(w, s, web_first);
        m.cross_edges.push(http_edge(e, r, HTTP_EMITTER, "exact"));
        m
    }

    /// `(from, to, evidence json, confidence)` of every SAME_AS cross edge.
    fn same_as_rows(m: &MergedGraph) -> Vec<(String, String, String, Confidence)> {
        let mut rows: Vec<_> = m
            .cross_edges
            .iter()
            .filter(|e| e.category == edge_category::SAME_AS)
            .map(|e| {
                let ev = match e.cells.as_slice() {
                    [
                        Cell {
                            kind,
                            payload: CellPayload::Json(j),
                        },
                    ] if *kind == cell_type::EVIDENCE => j.clone(),
                    other => panic!("one EVIDENCE cell, got {other:?}"),
                };
                (qname_in(m, e.from), qname_in(m, e.to), ev, e.confidence)
            })
            .collect();
        rows.sort_by(|a, b| (&a.0, &a.1, &a.2).cmp(&(&b.0, &b.1, &b.2)));
        rows
    }

    /// One exchange's confirmed pair is one `http_model` star across two
    /// repos, client -> server, DERIVED (Medium), located at the joining
    /// call site (0-based line), `/diverged` for a pair with drift.
    #[test]
    fn model_resolver_writes_one_star_per_server_model() {
        let (w, s) = exchange();
        let mut m = merged_of(w, s, true);
        let (edges, report) = model_identity(&m);
        let report = report.expect("an HTTP_CALLS pair: the markers print");
        assert_eq!(
            report.line(),
            "[models] classes=1 edges=1 diverged=1 ambiguous_client=0 ambiguous_project=0 \
             embed_reduced=0"
        );
        assert_eq!(report.wire_pairs, wire_pairs(&m).marker());
        assert_eq!(report.client_wires, client_wires(&m).marker());
        assert_eq!(edges.len(), 1);
        ModelResolver.resolve(&mut m);
        assert_eq!(
            same_as_rows(&m),
            [(
                "m::Order".to_string(),
                "models::m::Wide".to_string(),
                r#"{"emitter":"resolver:models","rule":"http_model/diverged","file":"a.ts","line":6,"basis":"site"}"#
                    .to_string(),
                Confidence::Medium,
            )]
        );
    }

    /// The client pairs `Wide` (GET /orders) and `Base` (GET /base), and
    /// `Wide` embeds `Base`: the embedded base is the class, in either graph
    /// order.
    #[test]
    fn a_client_pairing_a_struct_and_its_embedder_keeps_the_base() {
        let mut m = exchange_with_base(true);
        let (_, report) = model_identity(&m);
        let report = report.expect("markers");
        assert_eq!(
            report.line(),
            "[models] classes=1 edges=1 diverged=1 ambiguous_client=0 ambiguous_project=0 \
             embed_reduced=1"
        );
        ModelResolver.resolve(&mut m);
        let rows = same_as_rows(&m);
        assert_eq!(
            rows.iter()
                .map(|r| (r.0.as_str(), r.1.as_str()))
                .collect::<Vec<_>>(),
            [("m::Order", "models::m::Base")]
        );
        // Order's `items` is never written, Base's `title` never read.
        assert!(
            rows[0]
                .2
                .contains(r#""rule":"http_model/diverged","file":"a.ts","line":14"#),
            "{}",
            rows[0].2
        );

        let mut flipped = exchange_with_base(false);
        ModelResolver.resolve(&mut flipped);
        assert_eq!(same_as_rows(&flipped), rows, "graph order changes nothing");
    }

    /// No HTTP_CALLS pair, or no shaped hit: no edge and no marker.
    #[test]
    fn no_exchange_no_marker() {
        let (w, s) = exchange();
        let mut m = merged_of(w, s, true);
        m.cross_edges.clear();
        let (edges, report) = model_identity(&m);
        assert!(edges.is_empty() && report.is_none());
        let (edges, report) = model_identity(&MergedGraph::new(vec![]));
        assert!(edges.is_empty() && report.is_none());
    }

    // ---- DT.1n: the DT.1m request gap -------------------------------------

    /// A name the Go binder requires and the client marks optional is a 400
    /// waiting to happen; a response reads no binder.
    #[test]
    fn a_required_bind_the_client_may_omit_breaks_the_request() {
        let client = model_of(
            TS,
            1,
            false,
            false,
            vec![
                mf(TS, "discount", "string", false),
                mf(TS, "id", "string", true),
                mf(TS, "memo", "string", false),
            ],
        );
        let server = model_of(
            GO,
            11,
            false,
            false,
            vec![
                bound(mf(GO, "discount", "string", true)),
                bound(mf(GO, "id", "string", true)),
                mf(GO, "memo", "string", false),
            ],
        );
        let req = compare(WireDirection::Request, &client, &server, "");
        assert_eq!(changes(&req), [("discount", REQUIRED_MAY_BE_UNSET, true)]);
        let c = &req.changes[0];
        assert_eq!(
            (c.client.as_deref(), c.server.as_deref()),
            (Some("string"), Some("string"))
        );
        assert_eq!(
            req.models
                .iter()
                .map(|m| (m.client.0, m.server.0, m.diverged))
                .collect::<Vec<_>>(),
            [(1, 11, true)],
            "a requiredness disagreement diverges the pair"
        );
        let resp = compare(WireDirection::Response, &client, &server, "");
        assert!(changes(&resp).is_empty(), "{:?}", resp.changes);
    }

    // ---- DT.1n: GraphQL selection drift ------------------------------------

    /// A graphql SCHEMA_FIELDS cell of `(name, type)` fields.
    fn sdl(fields: &[(&str, &str)], open: bool) -> Cell {
        ModelSchema {
            fields: fields
                .iter()
                .map(|(n, t)| ModelField::new(n, t, t.ends_with('!')))
                .collect(),
            open,
            ..ModelSchema::new(WireFormat::Graphql)
        }
        .cell()
    }

    fn origin(decl: &str) -> Cell {
        Cell {
            kind: cell_type::ORIGIN,
            payload: CellPayload::Json(format!(
                r#"{{"provenance":"contract","source":"graphql","decl":"{decl}"}}"#
            )),
        }
    }

    fn returns(ty: &str) -> Cell {
        ServerWire::new(WireForm::GraphqlReturns, ty, 9, Shape::named(ty)).cell()
    }

    /// `doc`'s `graphql_selection` cell, read the way the extractor reads it.
    fn selection(raw: &str, doc: &str) -> Cell {
        let shape = glia_code_extractors::graphql::selection_shape(doc).expect("an operation");
        ServerWire::new(WireForm::GraphqlSelection, raw, 3, shape).cell()
    }

    fn gql_calls(from: NodeId, to: NodeId, emitter: &str) -> Edge {
        Edge::new(from, to, edge_category::GRAPHQL_CALLS, Confidence::Strong)
            .with_cell(Evidence::emitter(emitter).rule("root_field").to_cell())
    }

    /// The api's schema (Item, Owner, the interface Node, the extend-only
    /// Draft) and root fields, in one graph.
    fn schema_graph(tag: &str) -> G {
        let mut g = G::new(tag);
        let m = g.module("graph::schema", "graph/schema.graphqls");
        let item = sdl(
            &[
                ("id", "ID!"),
                ("label", "String!"),
                ("owner", "Owner!"),
                ("tags", "[String!]"),
            ],
            false,
        );
        g.node(
            node_kind::MESSAGE_TYPE,
            "message:graphql:Item @api",
            Some(m),
            vec![origin("type"), item],
        );
        let owner = sdl(&[("name", "String!")], false);
        g.node(
            node_kind::MESSAGE_TYPE,
            "message:graphql:Owner @api",
            Some(m),
            vec![origin("type"), owner],
        );
        let node = sdl(&[("id", "ID!")], false);
        g.node(
            node_kind::MESSAGE_TYPE,
            "message:graphql:Node @api",
            Some(m),
            vec![origin("interface"), node],
        );
        let draft = sdl(&[("id", "ID!")], true);
        g.node(
            node_kind::MESSAGE_TYPE,
            "message:graphql:Draft @api",
            Some(m),
            vec![origin("type"), draft],
        );
        for (field, ty) in [
            ("items", "[Item!]!"),
            ("adjustStock", "Item!"),
            ("node", "Node"),
            ("draft", "Draft"),
            ("search", "[SearchResult!]!"),
            ("count", "Int!"),
        ] {
            g.node(
                node_kind::GRAPHQL_RESOLVER,
                &format!("graphql_resolver:{field} @api"),
                Some(m),
                vec![returns(ty)],
            );
        }
        g
    }

    /// The console's ops, each paired with the root field it selects.
    const OPS: &[(&str, &str, &str, &str)] = &[
        (
            "ListItems",
            "query ListItems",
            "query ListItems($first: Int) { items(first: $first) { id label barcode owner { name email } } }",
            "items",
        ),
        (
            "AdjustStock",
            "mutation AdjustStock",
            "mutation AdjustStock { adjustStock(id: 1) { id label } }",
            "adjustStock",
        ),
        (
            "Aliased",
            "query Aliased",
            "query Aliased { first: items { code: barcode id } second: items { label owner { name } } }",
            "items",
        ),
        (
            "Node",
            "query Node",
            "query Node { node(id: 1) { id whatever } }",
            "node",
        ),
        (
            "Draft",
            "query Draft",
            "query Draft { draft { id title } }",
            "draft",
        ),
        (
            "Search",
            "query Search",
            "query Search { search { ... on Item { id } } }",
            "search",
        ),
        (
            "Spread",
            "query Spread",
            "query Spread { items { id barcode ...Elsewhere } }",
            "items",
        ),
        ("Count", "query Count", "query Count { count }", "count"),
    ];

    /// The schema graph, plus the console's ops in a second graph of the
    /// same repo, with their GRAPHQL_CALLS (`schema_first`: graph order).
    fn stock(schema_first: bool) -> MergedGraph {
        let s = schema_graph("stock");
        let mut c = G::new("stock");
        let m = c.module("console::documents", "console/documents.ts");
        let mut cross = Vec::new();
        for (name, raw, doc, field) in OPS {
            let op = c.node(
                node_kind::GRAPHQL_OPERATION,
                &format!("graphql_op:{name} @console"),
                Some(m),
                vec![selection(raw, doc)],
            );
            let to = NodeId::from_parts(
                GRAPH_TYPE,
                s.g.repo,
                node_kind::GRAPHQL_RESOLVER,
                &format!("graphql_resolver:{field} @api"),
            );
            cross.push(gql_calls(op, to, GRAPHQL_EMITTER));
        }
        let graphs = if schema_first {
            vec![s.g, c.g]
        } else {
            vec![c.g, s.g]
        };
        let mut merged = MergedGraph::new(graphs);
        merged.cross_edges = cross;
        merged
    }

    fn gql_rows(
        gp: &GraphqlPairs,
    ) -> Vec<(
        String,
        Vec<(String, String, String)>,
        Vec<&'static str>,
        Option<&'static str>,
    )> {
        gp.pairs
            .iter()
            .map(|p| {
                (
                    format!("{} {} {}", p.op_type, p.op_name, p.root_field),
                    p.changes
                        .iter()
                        .map(|c| (c.path.clone(), c.key.clone(), c.type_name.clone()))
                        .collect(),
                    p.notes.clone(),
                    p.refused,
                )
            })
            .collect()
    }

    fn ch(path: &str, key: &str, ty: &str) -> (String, String, String) {
        (path.to_string(), key.to_string(), ty.to_string())
    }

    #[test]
    fn graphql_selections_name_the_fields_their_type_lacks() {
        let gp = graphql_pairs(&stock(true));
        assert_eq!(
            gp.marker(),
            "[models] graphql pairs=7 undefined_fields=4 open=4 refused=0"
        );
        assert_eq!(
            gql_rows(&gp),
            [
                (
                    "mutation AdjustStock adjustStock".into(),
                    vec![],
                    vec![],
                    None
                ),
                (
                    "query Aliased items".into(),
                    vec![ch("barcode", "code", "Item")],
                    vec![],
                    None
                ),
                ("query Draft draft".into(), vec![], vec![NOTE_OPEN], None),
                (
                    "query ListItems items".into(),
                    vec![
                        ch("barcode", "barcode", "Item"),
                        ch("owner.email", "email", "Owner")
                    ],
                    vec![],
                    None
                ),
                ("query Node node".into(), vec![], vec![NOTE_OPEN], None),
                ("query Search search".into(), vec![], vec![NOTE_OPEN], None),
                (
                    "query Spread items".into(),
                    vec![ch("barcode", "barcode", "Item")],
                    vec![NOTE_OPEN],
                    None
                ),
            ],
            "Count selects a scalar root as a leaf: nothing to compare. Draft is extend-only (open), \
             Node an interface, SearchResult no type here, Spread's unknown spread leaves it open: \
             its listed barcode is still undefined"
        );
        let list = gp
            .pairs
            .iter()
            .find(|p| p.op_name == "ListItems")
            .expect("ListItems");
        assert_eq!(
            (
                list.returns.as_str(),
                list.type_name.as_str(),
                list.schema_type_qname.as_deref(),
                list.line,
                list.returns_line
            ),
            ("[Item!]!", "Item", Some("message:graphql:Item @api"), 3, 9)
        );
        assert!(
            list.changes
                .iter()
                .all(|c| c.breaking && c.rule == FIELD_UNDEFINED)
        );
        let search = gp
            .pairs
            .iter()
            .find(|p| p.op_name == "Search")
            .expect("Search");
        assert_eq!(
            (search.schema_type, search.type_name.as_str()),
            (None, "SearchResult")
        );
    }

    #[test]
    fn an_aliased_selection_is_judged_by_its_schema_field() {
        let gp = graphql_pairs(&stock(true));
        let p = gp
            .pairs
            .iter()
            .find(|p| p.op_name == "Aliased")
            .expect("Aliased");
        assert_eq!(p.response_keys, ["first", "second"]);
        let Shape::Object { fields, open } = &p.selection else {
            panic!("an object selection, got {:?}", p.selection);
        };
        assert!(!open);
        assert_eq!(
            fields
                .iter()
                .map(|f| (f.name.as_str(), f.field.as_deref()))
                .collect::<Vec<_>>(),
            [
                ("code", Some("barcode")),
                ("id", None),
                ("label", None),
                ("owner", None)
            ],
            "both response keys' sub-selections, merged"
        );
        assert_eq!(
            p.changes.len(),
            1,
            "id, label and owner {{ name }} are Item's"
        );
    }

    #[test]
    fn conflicting_copies_are_refused() {
        let mut merged = stock(true);
        let mut copy = G::new("stock");
        let m = copy.module("graph::other", "graph/other.graphqls");
        copy.node(
            node_kind::MESSAGE_TYPE,
            "message:graphql:Owner @api",
            Some(m),
            vec![sdl(&[("name", "String!"), ("email", "String")], false)],
        );
        copy.node(
            node_kind::GRAPHQL_RESOLVER,
            "graphql_resolver:adjustStock @api",
            Some(m),
            vec![returns("Owner!")],
        );
        let c = copy.module("console::again", "console/again.ts");
        copy.node(
            node_kind::GRAPHQL_OPERATION,
            "graphql_op:Aliased @console",
            Some(c),
            vec![selection("query Aliased", "query Aliased { items { id } }")],
        );
        merged.graphs.push(copy.g);
        let gp = graphql_pairs(&merged);
        let refused: Vec<(&str, Option<&'static str>)> = gp
            .pairs
            .iter()
            .map(|p| (p.op_name.as_str(), p.refused))
            .collect();
        assert_eq!(
            refused,
            [
                ("AdjustStock", Some(REFUSED_RETURNS)),
                ("Aliased", Some(REFUSED_SELECTION)),
                ("Draft", None),
                ("ListItems", None),
                ("Node", None),
                ("Search", None),
                ("Spread", None),
            ]
        );
        let list = gp
            .pairs
            .iter()
            .find(|p| p.op_name == "ListItems")
            .expect("ListItems");
        assert_eq!(
            list.changes
                .iter()
                .map(|c| c.path.as_str())
                .collect::<Vec<_>>(),
            ["barcode"],
            "Owner's two field lists: owner.email is not claimed"
        );
        assert_eq!(list.notes, [NOTE_OPEN]);

        // A root type whose two copies disagree refuses the pair.
        let mut merged = stock(true);
        let mut copy = G::new("stock");
        let m = copy.module("graph::other", "graph/other.graphqls");
        copy.node(
            node_kind::MESSAGE_TYPE,
            "message:graphql:Item @api",
            Some(m),
            vec![sdl(&[("id", "ID!")], false)],
        );
        merged.graphs.push(copy.g);
        let gp = graphql_pairs(&merged);
        let list = gp
            .pairs
            .iter()
            .find(|p| p.op_name == "ListItems")
            .expect("ListItems");
        assert_eq!(
            (list.refused, list.changes.len()),
            (Some(REFUSED_FIELDS), 0)
        );
        assert_eq!(
            list.schema_type_qname.as_deref(),
            Some("message:graphql:Item @api")
        );
        assert_eq!(gp.stats.refused, 4, "{}", gp.marker());
    }

    #[test]
    fn a_type_under_another_owner_is_refused() {
        let mut g = G::new("owners");
        let m = g.module("graph::schema", "graph/schema.graphqls");
        g.node(
            node_kind::MESSAGE_TYPE,
            "message:graphql:Item @other",
            Some(m),
            vec![sdl(&[("id", "ID!")], false)],
        );
        g.node(
            node_kind::MESSAGE_TYPE,
            "message:graphql:Thing",
            Some(m),
            vec![sdl(&[("id", "ID!")], false)],
        );
        let items = g.node(
            node_kind::GRAPHQL_RESOLVER,
            "graphql_resolver:items @api",
            Some(m),
            vec![returns("[Item!]!")],
        );
        let things = g.node(
            node_kind::GRAPHQL_RESOLVER,
            "graphql_resolver:things @api",
            Some(m),
            vec![returns("[Thing]")],
        );
        let bare = g.node(
            node_kind::GRAPHQL_RESOLVER,
            "graphql_resolver:thing",
            Some(m),
            vec![returns("Thing")],
        );
        let a = g.node(
            node_kind::GRAPHQL_OPERATION,
            "graphql_op:Items @web",
            Some(m),
            vec![selection("query Items", "query Items { items { id sku } }")],
        );
        let b = g.node(
            node_kind::GRAPHQL_OPERATION,
            "graphql_op:Things @web",
            Some(m),
            vec![selection(
                "query Things",
                "query Things { things { id sku } thing { id sku } }",
            )],
        );
        let c = g.node(
            node_kind::GRAPHQL_OPERATION,
            "graphql_op:Misnamed @web",
            Some(m),
            vec![selection(
                "query Misnamed",
                "query Misnamed { Items { id } }",
            )],
        );
        let mut merged = MergedGraph::new(vec![g.g]);
        merged.cross_edges = vec![
            gql_calls(a, items, GRAPHQL_EMITTER),
            gql_calls(b, things, GRAPHQL_EMITTER),
            gql_calls(b, bare, GRAPHQL_EMITTER),
            gql_calls(c, items, GRAPHQL_EMITTER),
            // An overlay's edge is not the resolver's pairing.
            gql_calls(c, bare, "overlay"),
        ];
        let gp = graphql_pairs(&merged);
        assert_eq!(
            gql_rows(&gp),
            [
                (
                    "query Items items".into(),
                    vec![],
                    vec![],
                    Some(REFUSED_OWNER)
                ),
                (
                    "query Misnamed items".into(),
                    vec![],
                    vec![],
                    Some(REFUSED_ROOT)
                ),
                (
                    "query Things thing".into(),
                    vec![ch("sku", "sku", "Thing")],
                    vec![],
                    None
                ),
                (
                    "query Things things".into(),
                    vec![],
                    vec![],
                    Some(REFUSED_OWNER)
                ),
            ],
            "Item lives under another owner; @api's Thing does not exist, the owner-free one \
             serves only the owner-free field; Misnamed's root key folds onto items but names \
             no `items` selection"
        );
        assert_eq!(
            gp.marker(),
            "[models] graphql pairs=4 undefined_fields=1 open=0 refused=3"
        );
    }

    #[test]
    fn graphql_pairs_are_deterministic_across_graph_order() {
        let a = graphql_pairs(&stock(true));
        assert_eq!(a, graphql_pairs(&stock(false)));
        assert_eq!(a, graphql_pairs(&stock(true)));
        assert_eq!(
            graphql_pairs(&MergedGraph::new(vec![])),
            GraphqlPairs::default()
        );
        assert_eq!(graphql_named("[[Item!]]!"), "Item");
    }

    // ---- DT.1l: the envelope-or-array union --------------------------------

    /// A client accepting `{ items: Item[] } | Item[]` is compared by the
    /// member of the server's kind: the envelope against an object write,
    /// the bare array against an array write; either way the model pairs.
    #[test]
    fn an_envelope_or_array_union_compares_its_member_of_the_servers_kind() {
        let item = |f: WireFormat, id: u64| {
            model_of(
                f,
                id,
                false,
                false,
                vec![mf(f, "id", "string", true), mf(f, "name", "string", true)],
            )
        };
        let mut arr = item(TS, 1);
        arr.array = true;
        let envelope = inline(
            TS,
            false,
            vec![fld("items", Some("Item[]"), true, arr.clone())],
        );
        let client = union_of(TS, vec![envelope, arr]);
        let mut server_arr = item(GO, 11);
        server_arr.array = true;
        let server_obj = inline(
            GO,
            false,
            vec![fld("items", None, true, server_arr.clone())],
        );
        for (server, path) in [(&server_obj, "items[]"), (&server_arr, "[]")] {
            let c = compare(WireDirection::Response, &client, server, "");
            assert!(changes(&c).is_empty(), "{path}: {:?}", c.changes);
            assert_eq!(
                c.models
                    .iter()
                    .map(|m| (m.client.0, m.server.0, m.path.as_str()))
                    .collect::<Vec<_>>(),
                [(1, 11, path)],
                "{path}"
            );
            assert!(c.notes.contains(NOTE_UNION));
        }
        // A member of no known kind keeps the union unjudged.
        let loose = union_of(TS, vec![client.members[0].clone(), val(TS, "any")]);
        let c = compare(WireDirection::Response, &loose, &server_obj, "");
        assert!(
            c.changes.is_empty() && c.models.is_empty(),
            "{:?}",
            c.changes
        );
        // A server scalar matches no member: nothing is judged.
        let c = compare(WireDirection::Response, &client, &val(GO, "string"), "");
        assert!(c.changes.is_empty() && c.models.is_empty());
        // Two array members against an array: no one member, nothing judged.
        let mut other = item(TS, 2);
        other.array = true;
        let mut first = item(TS, 1);
        first.array = true;
        let two = union_of(TS, vec![first, other]);
        let c = compare(WireDirection::Response, &two, &server_arr, "");
        assert!(c.models.is_empty(), "{:?}", c.models);
        // The envelope's drift is still named under its path.
        let thin = model_of(GO, 12, true, false, vec![mf(GO, "id", "string", true)]);
        let thin_obj = inline(GO, false, vec![fld("items", None, true, thin)]);
        let c = compare(WireDirection::Response, &client, &thin_obj, "");
        assert_eq!(changes(&c), [("items[].name", FIELD_MISSING, true)]);
    }

    // ---- DT.1k: graphql_model identity -------------------------------------

    const BANNER: &str =
        "// Code generated by github.com/99designs/gqlgen, DO NOT EDIT.\n\npackage model\n";

    fn span(file: &str, start: u32, end: u32) -> Cell {
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                r#"{{"file":"{file}","start_line":{start},"end_line":{end}}}"#
            )),
        }
    }

    /// A `graphql_result` site at 1-based `line` naming `doc`.
    fn result(doc: &str, line: u32, shape: Shape) -> Cell {
        let mut w = ServerWire::new(WireForm::GraphqlResult, "T", line, shape);
        w.document = Some(doc.to_string());
        w.cell()
    }

    /// `{ <key>: <ty> }`.
    fn keyed(key: &str, ty: &str) -> Shape {
        Shape::Object {
            fields: vec![ShapeField::new(key, true, Shape::named(ty))],
            open: false,
        }
    }

    fn code(text: &str) -> Cell {
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text.to_string()),
        }
    }

    const ITEM_FIELDS: &[(&str, &str, bool)] = &[
        ("id", "string", true),
        ("label", "string", true),
        ("priceCents", "int", true),
        ("onHand", "int", true),
    ];

    /// One console function: (name, document, 1-based call line, result).
    type Call<'a> = (&'a str, &'a str, u32, Shape);

    /// The models-graphql-stock shape. console: documents.ts holds
    /// LIST_ITEMS (rows `list`, minting ListItems at row 2), ADJUST_STOCK
    /// (rows 14-21, AdjustStock at 14) and ALIASED (rows 30-35, Aliased at
    /// 30, `stock: items`); types.ts InventoryItem (barcode beyond the
    /// type); inventory.ts one FUNCTION per `calls`, importing the three
    /// documents and InventoryItem. api: the SDL Item and its root fields
    /// (` @<owner>` on the resolvers when `owner`), gqlgen's generated
    /// model.Item and the hand-written stock.Item.
    fn gql_stock(calls: &[Call<'_>], list: (u32, u32), owner: Option<&str>) -> (G, G) {
        let mut c = G::new("console");
        let docs = c.module("src::documents", "src/documents.ts");
        let types = c.module("src::types", "src/types.ts");
        let inv = c.module("src::inventory", "src/inventory.ts");
        let item = c.node(
            node_kind::INTERFACE,
            "src::types::InventoryItem",
            Some(types),
            vec![
                span("src/types.ts", 0, 6),
                schema(
                    WireFormat::Ts,
                    &[
                        ("id", "string", true),
                        ("label", "string", true),
                        ("priceCents", "number", true),
                        ("onHand", "number", true),
                        ("barcode", "string", true),
                    ],
                    &[],
                ),
            ],
        );
        c.symbol(types, "InventoryItem", item);
        c.import(inv, "InventoryItem", item);
        let consts = [
            (
                "LIST_ITEMS",
                list,
                "ListItems",
                "query ListItems",
                "query ListItems { items { id label priceCents onHand barcode } }",
                2,
            ),
            (
                "ADJUST_STOCK",
                (14, 21),
                "AdjustStock",
                "mutation AdjustStock",
                "mutation AdjustStock { adjustStock(id: 1) { id onHand } }",
                14,
            ),
            (
                "ALIASED",
                (30, 35),
                "Aliased",
                "query Aliased",
                "query Aliased { stock: items { id label } }",
                30,
            ),
        ];
        for (name, (start, end), op, raw, doc, row) in consts {
            let v = c.node(
                node_kind::STATE_VAR,
                &format!("src::documents::{name}"),
                Some(docs),
                vec![span("src/documents.ts", start, end)],
            );
            c.symbol(docs, name, v);
            c.import(inv, name, v);
            c.node(
                node_kind::GRAPHQL_OPERATION,
                &format!("graphql_op:{op}"),
                None,
                vec![selection(raw, doc), span("src/documents.ts", row, row)],
            );
        }
        for (name, doc, line, shape) in calls {
            c.node(
                node_kind::FUNCTION,
                &format!("src::inventory::{name}"),
                Some(inv),
                vec![
                    span("src/inventory.ts", line - 1, line + 2),
                    result(doc, *line, shape.clone()),
                ],
            );
        }

        let mut a = G::new("api");
        let sm = a.module("graph::schema", "graph/schema.graphqls");
        a.node(
            node_kind::MESSAGE_TYPE,
            "message:graphql:Item",
            Some(sm),
            vec![
                span("graph/schema.graphqls", 0, 5),
                origin("type"),
                sdl(
                    &[
                        ("id", "ID!"),
                        ("label", "String!"),
                        ("priceCents", "Int!"),
                        ("onHand", "Int!"),
                    ],
                    false,
                ),
            ],
        );
        let at = |f: &str| match owner {
            Some(o) => format!("graphql_resolver:{f} @{o}"),
            None => format!("graphql_resolver:{f}"),
        };
        for (field, ty) in [("items", "[Item!]!"), ("adjustStock", "Item!")] {
            a.node(
                node_kind::GRAPHQL_RESOLVER,
                &at(field),
                Some(sm),
                vec![returns(ty)],
            );
        }
        let gen_mod = a.module("graph::model::models_gen", "graph/model/models_gen.go");
        a.cell(gen_mod, code(BANNER));
        a.node(
            node_kind::STRUCT,
            "graph::model::models_gen::Item",
            Some(gen_mod),
            vec![
                span("graph/model/models_gen.go", 4, 9),
                schema(WireFormat::Go, ITEM_FIELDS, &[]),
            ],
        );
        let stock_mod = a.module("internal::stock::item", "internal/stock/item.go");
        a.cell(
            stock_mod,
            code("package stock\n\n// Item is a warehouse row.\ntype Item struct {}\n"),
        );
        a.node(
            node_kind::STRUCT,
            "internal::stock::item::Item",
            Some(stock_mod),
            vec![
                span("internal/stock/item.go", 4, 9),
                schema(WireFormat::Go, ITEM_FIELDS, &[]),
            ],
        );
        (c, a)
    }

    /// The GRAPHQL_CALLS the graphql resolver draws over [`gql_stock`].
    fn gql_merged(c: G, a: G, owner: Option<&str>, console_first: bool) -> MergedGraph {
        let (crepo, arepo) = (c.g.repo, a.g.repo);
        let at = |f: &str| match owner {
            Some(o) => format!("graphql_resolver:{f} @{o}"),
            None => format!("graphql_resolver:{f}"),
        };
        let edge = |op: &str, field: &str| {
            gql_calls(
                NodeId::from_parts(
                    GRAPH_TYPE,
                    crepo,
                    node_kind::GRAPHQL_OPERATION,
                    &format!("graphql_op:{op}"),
                ),
                NodeId::from_parts(GRAPH_TYPE, arepo, node_kind::GRAPHQL_RESOLVER, &at(field)),
                GRAPHQL_EMITTER,
            )
        };
        let cross = vec![
            edge("ListItems", "items"),
            edge("AdjustStock", "adjustStock"),
            edge("Aliased", "items"),
        ];
        let graphs = if console_first {
            vec![c.g, a.g]
        } else {
            vec![a.g, c.g]
        };
        let mut m = MergedGraph::new(graphs);
        m.cross_edges = cross;
        m
    }

    fn fixture_calls() -> Vec<Call<'static>> {
        vec![
            (
                "emptyShelves",
                "LIST_ITEMS",
                6,
                keyed("items", "InventoryItem[]"),
            ),
            (
                "adjust",
                "ADJUST_STOCK",
                11,
                keyed("adjustStock", "InventoryItem"),
            ),
        ]
    }

    /// `(from, to, evidence)` of every `graphql_model` SAME_AS edge.
    fn gql_same_as(m: &MergedGraph) -> Vec<(String, String, String)> {
        same_as_rows(m)
            .into_iter()
            .filter(|r| r.2.contains("\"rule\":\"graphql_model"))
            .map(|r| {
                assert_eq!(r.3, Confidence::Medium, "DERIVED");
                (r.0, r.1, r.2)
            })
            .collect()
    }

    fn ev(rule: &str, file: &str, line: u32) -> String {
        format!(
            r#"{{"emitter":"resolver:models","rule":"{rule}","file":"{file}","line":{line},"basis":"site"}}"#
        )
    }

    /// The fixture: InventoryItem (barcode beyond the type: diverged) and
    /// gqlgen's generated model.Item join message:graphql:Item, the client
    /// edge at the first join by op qname (AdjustStock's call, 0-based
    /// line 10), the generated one at its declaration; the hand-written
    /// stock.Item joins nothing. Graph order changes nothing.
    #[test]
    fn graphql_model_joins_the_result_model_and_the_generated_model() {
        let (c, a) = gql_stock(&fixture_calls(), (2, 12), None);
        let mut m = gql_merged(c, a, None, true);
        let (edges, report) = graphql_model_identity(&m);
        let report = report.expect("graphql_result sites: the marker prints");
        assert_eq!(
            report.line(),
            "[models] graphql results=2 documents_resolved=2 generated_models=1 classes=1"
        );
        assert_eq!(edges.len(), 2);
        ModelResolver.resolve(&mut m);
        let rows = gql_same_as(&m);
        assert_eq!(
            rows,
            [
                (
                    "graph::model::models_gen::Item".to_string(),
                    "message:graphql:Item".to_string(),
                    ev("graphql_model", "graph/model/models_gen.go", 4),
                ),
                (
                    "src::types::InventoryItem".to_string(),
                    "message:graphql:Item".to_string(),
                    ev("graphql_model/diverged", "src/inventory.ts", 10),
                ),
            ]
        );
        let (c, a) = gql_stock(&fixture_calls(), (2, 12), None);
        let mut flipped = gql_merged(c, a, None, false);
        ModelResolver.resolve(&mut flipped);
        assert_eq!(gql_same_as(&flipped), rows, "graph order changes nothing");
    }

    /// An alias's response key reaches its schema field (`stock: items`):
    /// one call through ALIASED alone forms the class; a model that lists
    /// only the type's names is not diverged.
    #[test]
    fn an_aliased_result_key_reaches_its_root_field() {
        let calls = [("aliased", "ALIASED", 32, keyed("stock", "InventoryItem[]"))];
        let (mut c, a) = gql_stock(&calls, (2, 12), None);
        // A model without barcode: the type's names only.
        let types = NodeId::from_parts(GRAPH_TYPE, c.g.repo, node_kind::MODULE, "src::types");
        let exact = c.node(
            node_kind::INTERFACE,
            "src::types::ExactItem",
            Some(types),
            vec![
                span("src/types.ts", 8, 12),
                schema(
                    WireFormat::Ts,
                    &[("id", "string", true), ("label", "string", true)],
                    &[],
                ),
            ],
        );
        let inv = NodeId::from_parts(GRAPH_TYPE, c.g.repo, node_kind::MODULE, "src::inventory");
        c.import(inv, "ExactItem", exact);
        let fn_id = NodeId::from_parts(
            GRAPH_TYPE,
            c.g.repo,
            node_kind::FUNCTION,
            "src::inventory::aliased",
        );
        c.cell(fn_id, result("ALIASED", 33, keyed("stock", "ExactItem")));
        let mut m = gql_merged(c, a, None, true);
        let (_, report) = graphql_model_identity(&m);
        // Two client models of one project pair Item: both dropped.
        assert_eq!(
            report.map(|r| r.line()).as_deref(),
            Some("[models] graphql results=2 documents_resolved=2 generated_models=0 classes=0")
        );
        ModelResolver.resolve(&mut m);
        assert!(gql_same_as(&m).is_empty());

        let calls = [("aliased", "ALIASED", 32, keyed("stock", "ExactItem"))];
        let (mut c, a) = gql_stock(&calls, (2, 12), None);
        let exact = c.node(
            node_kind::INTERFACE,
            "src::types::ExactItem",
            Some(types),
            vec![
                span("src/types.ts", 8, 12),
                schema(
                    WireFormat::Ts,
                    &[("id", "string", true), ("label", "string", true)],
                    &[],
                ),
            ],
        );
        c.import(inv, "ExactItem", exact);
        let mut m = gql_merged(c, a, None, true);
        ModelResolver.resolve(&mut m);
        let rows = gql_same_as(&m);
        assert_eq!(
            rows.iter()
                .map(|r| (r.0.as_str(), r.2.contains("/diverged")))
                .collect::<Vec<_>>(),
            [
                ("graph::model::models_gen::Item", false),
                ("src::types::ExactItem", false)
            ]
        );
    }

    /// One client model per project: two of one project pairing Item are both
    /// dropped (no class, so no generated member either); in two projects
    /// both join.
    #[test]
    fn graphql_model_keeps_one_client_model_per_project() {
        let build = |two_projects: bool| {
            let (mut c, a) = gql_stock(&fixture_calls(), (2, 12), None);
            let admin_types = c.module("admin::src::types", "admin/src/types.ts");
            let row = c.node(
                node_kind::INTERFACE,
                "admin::src::types::StockRow",
                Some(admin_types),
                vec![
                    span("admin/src/types.ts", 0, 5),
                    schema(
                        WireFormat::Ts,
                        &[("id", "string", true), ("onHand", "number", true)],
                        &[],
                    ),
                ],
            );
            let admin = c.module("admin::src::rows", "admin/src/rows.ts");
            let list = NodeId::from_parts(
                GRAPH_TYPE,
                c.g.repo,
                node_kind::STATE_VAR,
                "src::documents::LIST_ITEMS",
            );
            c.import(admin, "LIST_ITEMS", list);
            c.import(admin, "StockRow", row);
            c.node(
                node_kind::FUNCTION,
                "admin::src::rows::rows",
                Some(admin),
                vec![
                    span("admin/src/rows.ts", 2, 5),
                    result("LIST_ITEMS", 3, keyed("items", "StockRow[]")),
                ],
            );
            let origin = |path: &str| Cell {
                kind: cell_type::ORIGIN,
                payload: CellPayload::Json(format!(
                    r#"{{"label":"x","path":"{path}","provenance":"project_root"}}"#
                )),
            };
            c.node(node_kind::PROJECT, "project:.", None, vec![origin("")]);
            if two_projects {
                c.node(
                    node_kind::PROJECT,
                    "project:admin",
                    None,
                    vec![origin("admin")],
                );
            }
            let mut m = gql_merged(c, a, None, true);
            ModelResolver.resolve(&mut m);
            gql_same_as(&m).into_iter().map(|r| r.0).collect::<Vec<_>>()
        };
        assert!(build(false).is_empty(), "one project, two models: none");
        assert_eq!(
            build(true),
            [
                "admin::src::types::StockRow",
                "graph::model::models_gen::Item",
                "src::types::InventoryItem"
            ]
        );
    }

    /// A const holding two operations names neither; a type only under
    /// another owner than its root field's is refused (DT.1n's
    /// owner_mismatch); with no GRAPHQL_CALLS the marker still counts the
    /// sites; with no site there is no marker.
    #[test]
    fn graphql_model_refuses_what_it_cannot_pin() {
        let empty = [(
            "emptyShelves",
            "LIST_ITEMS",
            6,
            keyed("items", "InventoryItem[]"),
        )];
        let (c, a) = gql_stock(&empty, (2, 16), None);
        let m = gql_merged(c, a, None, true);
        let (edges, report) = graphql_model_identity(&m);
        assert!(edges.is_empty());
        assert_eq!(
            report.map(|r| r.line()).as_deref(),
            Some("[models] graphql results=1 documents_resolved=1 generated_models=0 classes=0"),
            "LIST_ITEMS' rows 2-16 hold ListItems and AdjustStock"
        );

        let (c, a) = gql_stock(&fixture_calls(), (2, 12), Some("api"));
        let m = gql_merged(c, a, Some("api"), true);
        assert!(
            graphql_pairs(&m)
                .pairs
                .iter()
                .all(|p| p.refused == Some(REFUSED_OWNER)),
            "message:graphql:Item has no @api copy"
        );
        let (edges, report) = graphql_model_identity(&m);
        assert!(edges.is_empty());
        assert_eq!(report.map(|r| r.classes), Some(0));

        let (c, a) = gql_stock(&fixture_calls(), (2, 12), None);
        let mut m = gql_merged(c, a, None, true);
        m.cross_edges.clear();
        let (edges, report) = graphql_model_identity(&m);
        assert!(edges.is_empty());
        assert_eq!(
            report.map(|r| r.line()).as_deref(),
            Some("[models] graphql results=2 documents_resolved=2 generated_models=0 classes=0")
        );

        let (c, a) = gql_stock(&[], (2, 12), None);
        let m = gql_merged(c, a, None, true);
        assert_eq!(graphql_model_identity(&m), (Vec::new(), None));
        let unbound = [(
            "emptyShelves",
            "NOT_IMPORTED",
            6,
            keyed("items", "InventoryItem[]"),
        )];
        let (c, a) = gql_stock(&unbound, (2, 12), None);
        let (_, report) = graphql_model_identity(&gql_merged(c, a, None, true));
        assert_eq!(
            report.map(|r| (r.results, r.documents_resolved)),
            Some((1, 0))
        );
    }

    /// The generated member: a generator header on the file's MODULE, a
    /// codegen file name, or an ORIGIN `generated` already stamped (a
    /// merge); never a protobuf file, a hand-written struct, or one of two
    /// generated structs of the type's name in one project.
    #[test]
    fn the_generated_member_is_the_one_generated_struct_of_the_types_name() {
        let members = |extra: &dyn Fn(&mut G)| {
            let (c, mut a) = gql_stock(&fixture_calls(), (2, 12), None);
            extra(&mut a);
            let mut m = gql_merged(c, a, None, true);
            ModelResolver.resolve(&mut m);
            gql_same_as(&m).into_iter().map(|r| r.0).collect::<Vec<_>>()
        };
        assert_eq!(
            members(&|_| {}),
            [
                "graph::model::models_gen::Item",
                "src::types::InventoryItem"
            ]
        );
        // A second generated Item of the project: neither joins.
        assert_eq!(
            members(&|a: &mut G| {
                let m2 = a.module("graph::model2::models_gen", "graph/model2/models_gen.go");
                a.cell(m2, code(BANNER));
                a.node(
                    node_kind::STRUCT,
                    "graph::model2::models_gen::Item",
                    Some(m2),
                    vec![
                        span("graph/model2/models_gen.go", 4, 9),
                        schema(WireFormat::Go, ITEM_FIELDS, &[]),
                    ],
                );
            }),
            ["src::types::InventoryItem"]
        );
        let generated = |file: &str, cells: Vec<Cell>, module_code: &str| {
            let (_, mut a) = gql_stock(&[], (2, 12), None);
            let md = a.module("x::m", file);
            a.cell(md, code(module_code));
            let mut all = vec![span(file, 1, 2)];
            all.extend(cells);
            let id = a.node(node_kind::STRUCT, "x::m::Item", Some(md), all);
            let ix = GraphIndex::new(&a.g);
            let node = a.g.nodes.iter().find(|n| n.id == id).cloned().unwrap();
            is_generated_struct(&ix, &node)
        };
        let stamped = Cell {
            kind: cell_type::ORIGIN,
            payload: CellPayload::Json(r#"{"provenance":"generated"}"#.to_string()),
        };
        assert!(
            generated("x/models_gen.go", vec![], BANNER),
            "a generator header"
        );
        assert!(
            generated("x/Models.Designer.cs", vec![], "class M {}"),
            "a codegen name"
        );
        assert!(
            generated("x/m.go", vec![stamped], "package m"),
            "a stamped ORIGIN"
        );
        assert!(
            !generated("x/m.pb.go", vec![], BANNER),
            "protobuf output is generated_proto"
        );
        assert!(!generated(
            "x/m.go",
            vec![],
            "package m\n// Item is hand-written.\n"
        ));
    }

    /// DT.1q's barrels: a document named through a barrel's re-export (the
    /// import binding DT.1q persists) or `ns.NAME` on a namespace import of
    /// the barrel (its same-name re-export in the barrel's symbols) resolves
    /// to its STATE_VAR.
    #[test]
    fn a_document_imported_through_a_barrel_resolves_to_its_state_var() {
        for doc in ["LIST_ITEMS", "gql.LIST_ITEMS"] {
            let (mut c, a) = gql_stock(&[], (2, 12), None);
            let list = NodeId::from_parts(
                GRAPH_TYPE,
                c.g.repo,
                node_kind::STATE_VAR,
                "src::documents::LIST_ITEMS",
            );
            let item = NodeId::from_parts(
                GRAPH_TYPE,
                c.g.repo,
                node_kind::INTERFACE,
                "src::types::InventoryItem",
            );
            let barrel = c.module("src::graphql", "src/graphql/index.ts");
            c.symbol(barrel, "LIST_ITEMS", list);
            let page = c.module("src::page", "src/page.ts");
            c.import(page, "LIST_ITEMS", list);
            c.import(page, "gql", barrel);
            c.import(page, "InventoryItem", item);
            c.node(
                node_kind::FUNCTION,
                "src::page::load",
                Some(page),
                vec![
                    span("src/page.ts", 3, 6),
                    result(doc, 4, keyed("items", "InventoryItem[]")),
                ],
            );
            assert_eq!(bind_client_name(&c.g, page, doc), Some(list), "{doc}");
            let mut m = gql_merged(c, a, None, true);
            let (_, report) = graphql_model_identity(&m);
            assert_eq!(
                report.map(|r| (r.documents_resolved, r.classes)),
                Some((1, 1)),
                "{doc}"
            );
            ModelResolver.resolve(&mut m);
            assert_eq!(gql_same_as(&m).len(), 2, "{doc}");
        }
    }

    // ---- DT.1o: one side flattened ----------------------------------------

    fn keys(w: &WireFields) -> Vec<&str> {
        w.fields.keys().map(String::as_str).collect()
    }

    fn opens(w: &WireFields) -> Vec<&str> {
        w.open.iter().map(String::as_str).collect()
    }

    #[test]
    fn wire_fields_flatten_nested_models_by_json_path() {
        let order = model_of(
            TS,
            1,
            false,
            false,
            vec![
                mf(TS, "id", "string", true),
                fld(
                    "lines",
                    Some("OrderLine[]"),
                    true,
                    model_of(
                        TS,
                        2,
                        true,
                        false,
                        vec![mf(TS, "productId", "string", true)],
                    ),
                ),
                fld(
                    "owner",
                    Some("Owner"),
                    false,
                    model_of(TS, 3, false, false, vec![mf(TS, "email", "string", true)]),
                ),
                mf(TS, "tags", "string[]", true),
            ],
        );
        let w = wire_fields(&order);
        assert_eq!(
            keys(&w),
            [
                "",
                "id",
                "lines",
                "lines[].productId",
                "owner",
                "owner.email",
                "tags"
            ]
        );
        assert!(w.open.is_empty(), "{:?}", w.open);
        let root = &w.fields[""];
        assert_eq!(
            (root.kind, root.model, root.required),
            (Some("object"), Some(NodeId(1)), true)
        );
        let lines = &w.fields["lines"];
        assert_eq!(
            (lines.kind, lines.array, lines.model, lines.ty.as_str()),
            (Some("array"), true, Some(NodeId(2)), "OrderLine[]")
        );
        let owner = &w.fields["owner"];
        assert_eq!(
            (owner.kind, owner.required),
            (Some("object"), false),
            "an unknown name that resolved to a model is an object"
        );
        assert_eq!(w.fields["owner.email"].kind, Some("string"));
    }

    #[test]
    fn a_root_array_lists_its_elements_unprefixed_and_an_envelope_its_key() {
        let list = model_of(TS, 1, true, false, vec![mf(TS, "id", "string", true)]);
        let w = wire_fields(&list);
        assert_eq!(keys(&w), ["", "id"]);
        assert_eq!(
            (w.fields[""].kind, w.fields[""].array),
            (Some("array"), true)
        );
        let envelope = inline(
            GO,
            false,
            vec![fld(
                "invoices",
                None,
                true,
                model_of(
                    GO,
                    4,
                    true,
                    false,
                    vec![mf(GO, "customerName", "string", true)],
                ),
            )],
        );
        let w = wire_fields(&envelope);
        assert_eq!(keys(&w), ["", "invoices", "invoices[].customerName"]);
        assert_eq!(w.fields["invoices"].kind, Some("array"));
        assert_eq!(w.fields[""].kind, Some("object"));
    }

    #[test]
    fn wire_fields_mark_the_paths_whose_names_are_unknown() {
        let side = model_of(
            TS,
            1,
            false,
            false,
            vec![
                mf(TS, "meta", "Record<string, string>", true),
                mf(TS, "status", "OrderStatus", true),
                mf(TS, "tags", "string[]", true),
                fld(
                    "extra",
                    Some("Extra"),
                    true,
                    model_of(TS, 5, false, true, vec![mf(TS, "a", "string", true)]),
                ),
                fld(
                    "parent",
                    Some("Order"),
                    false,
                    model_of(TS, 1, false, false, vec![mf(TS, "id", "string", true)]),
                ),
                fld(
                    "either",
                    None,
                    true,
                    union_of(TS, vec![val(TS, "string"), val(TS, "number")]),
                ),
            ],
        );
        let w = wire_fields(&side);
        assert_eq!(
            opens(&w),
            ["either", "extra", "meta", "parent", "status"],
            "an object no model lists, a name bound to nothing, an open model, a union and a \
             model met again on its own path"
        );
        assert!(
            w.fields.contains_key("extra.a") && !w.fields.contains_key("parent.id"),
            "an open model still lists its names; a model on its own path is not re-entered"
        );
        assert_eq!(w.fields["status"].kind, None, "an unbound name has no kind");
        assert_eq!(
            opens(&wire_fields(&ResolvedShape {
                written: Shape::open("res.body"),
                format: Some(TS),
                model: None,
                array: false,
                fields: BTreeMap::new(),
                members: Vec::new(),
                open: true,
            })),
            [""],
            "an open root opens the whole side"
        );
    }

    #[test]
    fn wire_fields_keep_the_member_the_declaring_base_and_a_codecs_direction() {
        let mut renamed = mf(GO, "customer_name", "string", true);
        renamed.member = Some("CustomerName".into());
        renamed.declared_in = Some(NodeId(9));
        let w = wire_fields(&model_of(GO, 9, false, false, vec![renamed]));
        let f = &w.fields["customer_name"];
        assert_eq!(
            (f.name.as_str(), f.member.as_str(), f.declared_by),
            ("customer_name", "CustomerName", Some(NodeId(9)))
        );
        let (s, ids) = server_graph();
        let merged = MergedGraph::new(vec![s.g]);
        let ctx = Ctx::new(&merged);
        let resolve = |q: &str, dir| {
            let shape = modeled("models.X", q);
            wire_fields(&ctx.resolve(0, Names::Written, &shape, Some(GO), dir, 0))
        };
        let wide = resolve("models::m::Wide", WireDirection::Response);
        assert_eq!(
            wide.fields["id"].declared_by,
            Some(ids["base"]),
            "an extends base's field"
        );
        assert_eq!(wide.fields["title"].declared_by, Some(ids["wide"]));
        assert_eq!(
            wide.fields["title"].member, "title",
            "no `field` recorded: the member is the JSON name"
        );
        assert_eq!(
            opens(&resolve("models::m::Geo", WireDirection::Response)),
            [""],
            "MarshalJSON writes what it likes"
        );
        assert!(
            resolve("models::m::Geo", WireDirection::Request)
                .open
                .is_empty()
        );
        assert_eq!(
            opens(&resolve("models::m::Decoded", WireDirection::Request)),
            [""]
        );
    }

    #[test]
    fn graphql_fields_key_both_sides_by_schema_field_path() {
        let m = stock(true);
        let gp = graphql_pairs(&m);
        let sides = graphql_fields_all(&m, &gp.pairs);
        assert_eq!(sides.len(), gp.pairs.len());
        let of = |name: &str| {
            let i = gp
                .pairs
                .iter()
                .position(|p| p.op_name == name)
                .unwrap_or_else(|| panic!("{name}"));
            (&gp.pairs[i], &sides[i])
        };
        let (aliased, (sel, served)) = of("Aliased");
        assert_eq!(
            keys(sel),
            ["", "barcode", "id", "label", "owner", "owner.name"],
            "two response keys of one root field merged"
        );
        let code = &sel.fields["barcode"];
        assert_eq!(
            (
                code.name.as_str(),
                code.member.as_str(),
                code.required,
                code.kind
            ),
            ("code", "barcode", true, None),
            "an alias keys its schema field"
        );
        assert_eq!(
            keys(served),
            ["", "id", "label", "owner", "owner.name", "tags"],
            "Item's fields, recursing into Owner where the selection does"
        );
        let item = aliased.schema_type;
        assert_eq!(served.fields["id"].declared_by, item);
        assert_eq!(
            (
                served.fields["owner"].kind,
                served.fields["owner"].ty.as_str()
            ),
            (Some("object"), "Owner!")
        );
        assert_ne!(
            served.fields["owner.name"].declared_by, item,
            "Owner declares it"
        );
        assert_eq!(
            (
                served.fields[""].kind,
                served.fields[""].required,
                served.fields[""].ty.as_str()
            ),
            (Some("array"), true, "[Item!]!")
        );
        assert_eq!(served.fields["tags"].required, false, "nullable");
        assert!(sel.open.is_empty() && served.open.is_empty());
        assert_eq!(
            graphql_fields(&m, aliased),
            (sel.clone(), served.clone()),
            "one pair alone reads the same"
        );

        let (_, (sel, _)) = of("Spread");
        assert_eq!(opens(sel), [""], "a spread opens the selection");
        let (_, (_, served)) = of("Draft");
        assert_eq!(opens(served), [""], "an extend-only type is open");
        let (_, (_, served)) = of("Node");
        assert_eq!(opens(served), [""], "an interface is open");
        let (_, (_, served)) = of("Search");
        assert_eq!(opens(served), [""], "no schema type: the side is open");
        assert!(graphql_fields_all(&m, &[]).is_empty());
    }
}
