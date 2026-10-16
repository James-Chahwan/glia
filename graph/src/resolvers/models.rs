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
//! now, the models resolver from DT.1h):
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
//! requires it, else `json_field_unset`. Both: JSON kinds that differ while
//! both are known, array against scalar included, are `json_type_mismatch`
//! (breaking); an unknown name is an object only when it resolved to a
//! model, else (like any / null, a call no builder typed, an open value)
//! it is never judged. No name is claimed absent from an open side (note
//! `open`); a union whose members are all objects is compared as their
//! merged fields (note `union`). Paths flatten as `items[].sku`. Arrays and
//! same-named fields recurse to [`MAX_DEPTH`], under inline roots too
//! (`{ listings: Listing[] }` against `gin.H{"listings": ..}`). Where both
//! sides are models with listed fields and [`confirms`] holds (at least one
//! shared name, and at least half of the smaller side's) the two are a
//! [`ModelPair`], diverged when a change sits at that model's level (a
//! nested model level owns its own). Read by DT.1h (identity), DT.1i (drift
//! rows in contract_fields).
//!
//! fired_on ([`WirePairs::marker`], printed by the engine test now, by the
//! models resolver from DT.1h and by contract_fields from DT.1i):
//! `[models] pairs=<p> response=<r> request=<q> model_pairs=<m> changes=<c>
//! breaking=<b> ambiguous_server=<a> open=<o>`.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use glia_code_domain::endpoint::{http_node_span, split_owner};
use glia_code_domain::evidence::{Evidence, locate};
use glia_code_domain::wire::{
    JsonKind, ModelField, ModelSchema, ServerWire, Shape, WireForm, WireFormat, element_type,
    hit_shapes, json_kind,
};
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, NodeId, NodeKindId};

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
    /// `json_required_missing`, `json_field_unset`, `json_type_mismatch` (a
    /// request).
    pub rule: &'static str,
    /// `json_field_missing`, `json_required_missing` and
    /// `json_type_mismatch` break the exchange.
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
    let cw = client_wires(merged);
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
}
