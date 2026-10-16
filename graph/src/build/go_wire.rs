//! DT.1f: the Go build resolves the wire cells the Go parser wrote (DT.1e).
//!
//! A handler's JSON response write or JSON request bind is a
//! [`ServerWire`] MESSAGE_TYPE cell on the handler, its value classified as
//! written: a type, a call chain, a field chain, an envelope's keys. Only the
//! Go build can say which in-repo STRUCT such a value is: the facts that type
//! `h.store.Orders(..)` ([`CodeNav`](glia_code_domain::CodeNav)'s
//! `local_types` / `field_types` / `return_types` and
//! [`NavFact::SliceResult`]) are build-time only and never persisted, so a
//! cross-graph pass or a layout merge cannot. [`resolve`] types each value
//! through [`GoPackages`]' receiver machinery (CA.2b), the one CALLS already
//! binds typed receivers with, and writes what it found into the cells, so
//! the shape is persisted and the models pass reading it is merge-safe:
//!
//! - **Named** (`models.Order`): the type text through the handler's file's
//!   imports ([`GoPackages::type_of_text`]).
//! - **Call** (`self.store.Orders()`): the callee of the last call
//!   ([`Wire::callee`]), then its declared first result
//!   (`return_types`, read in the callee's own file) or its
//!   [`NavFact::SliceResult`] (an array of the element).
//! - **Chain** (`self.cfg.Defaults`): the field's declared type text in its
//!   owner's SCHEMA_FIELDS (arrays too), else [`GoPackages::chain_type`].
//! - **Object** (an envelope, an inline struct): each key's value, nesting
//!   at most [`MAX_DEPTH`] deep. A **Union** resolves each member.
//!
//! A value resolves only to a STRUCT: an INTERFACE result, an external type,
//! a generic instantiation and an ambiguous name stay unresolved and are
//! counted. A scalar (`string`, `int`, `map[..]..`; [`json_kind`] knows its
//! kind) is no model and is not counted.
//!
//! What it writes:
//!
//! - The cell, rewritten through [`ServerWire::to_payload`] (fixed top-level
//!   order, never a `Value` round trip): each resolved shape gains `model`
//!   (and `array` from a SliceResult); a resolved top-level value sets the
//!   A12.1 `type` to the struct's simple name.
//! - One handler -USES-> STRUCT edge per (handler, struct, form), EVIDENCE
//!   `graph:go_wire` rule `json_response` / `json_bind` at the site's 0-based
//!   line. Confidence Strong when the struct came from a written type (a
//!   Named value, an inline object's typed key) and Medium when it came
//!   through call or chain typing (`glia why` tiers that DERIVED: an inferred
//!   graph binding); of several sites for one key the strongest, then the
//!   first, places the edge. A json_response whose status is unknown
//!   (`status` absent: a variable, a call) resolves its cell but draws no
//!   edge; a json_bind never carries a status and always draws one.
//! - Every STRUCT's SCHEMA_FIELDS (format `go`) field whose element type
//!   types to an in-repo STRUCT gains that STRUCT's qname as `model`
//!   (through [`ModelSchema::to_payload`], sorted and byte-stable).
//!
//! Deterministic: nodes are visited in `g.nodes` order, cells in their
//! order, shapes depth first; edges are pushed in that order and every map is
//! only probed, so no map iteration reaches the output.
//!
//! fired_on, once per Go graph with a wire cell or a nested model:
//! `[go-wire] resolved sites=<s>/<n> models=<m> (type=<t> call=<c> chain=<h>)
//! unresolved=<u> uses=<e> nested=<k>`.

use std::collections::HashMap;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::wire::{
    JsonKind, ModelSchema, ServerWire, Shape, WireForm, WireFormat, element_type, json_kind,
};
use glia_code_domain::{NavFact, cell_type, edge_category, node_kind};
use glia_core::{CellPayload, Confidence, NodeId};

use super::{GoPackages, RecvScope, Seg, Typed, is_exported, receiver_segments};
use crate::calls::push_edge_with;
use crate::types::RepoGraph;

/// The emitter of every edge this pass draws.
const EMITTER: &str = "graph:go_wire";

/// How deep an inline object's keys are resolved: the site's value is depth
/// 1, and the Go parser writes envelopes at most three deep (its
/// `WIRE_DEPTH`), so an Object at depth 3 still resolves its keys.
const MAX_DEPTH: usize = 3;

/// What [`resolve`] did to one Go graph, for the `[go-wire] resolved`
/// marker.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct WireStats {
    /// json_response / json_bind ServerWire cells read.
    sites: usize,
    /// Of `sites`: at least one shape resolved to a STRUCT.
    resolved_sites: usize,
    /// Shapes given a `model`, over every site.
    models: usize,
    /// Of `models`: through a written type (Named).
    by_type: usize,
    /// Of `models`: through a call's declared result.
    by_call: usize,
    /// Of `models`: through a field chain.
    by_chain: usize,
    /// Shapes that could hold a model but resolved to no STRUCT (an INTERFACE
    /// result, an external type, an open expression such as `len(xs)`).
    unresolved: usize,
    /// USES edges pushed.
    uses: usize,
    /// SCHEMA_FIELDS fields given a `model`.
    nested: usize,
}

impl WireStats {
    /// The fired_on line, `None` for a graph with no wire cell and no nested
    /// model.
    pub(super) fn marker(&self) -> Option<String> {
        (self.sites + self.nested > 0).then(|| {
            format!(
                "[go-wire] resolved sites={}/{} models={} (type={} call={} chain={}) \
                 unresolved={} uses={} nested={}",
                self.resolved_sites,
                self.sites,
                self.models,
                self.by_type,
                self.by_call,
                self.by_chain,
                self.unresolved,
                self.uses,
                self.nested
            )
        })
    }
}

/// Which shape typed a model: the confidence of the edge it draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    Type,
    Call,
    Chain,
}

impl Via {
    fn confidence(self) -> Confidence {
        match self {
            Via::Type => Confidence::Strong,
            Via::Call | Via::Chain => Confidence::Medium,
        }
    }
}

/// What a field chain reads ([`Wire::chain`]).
enum Field {
    /// A STRUCT, and whether the field is an array of it.
    Model(NodeId, bool),
    /// A field whose declared type is a scalar (`string`, `time.Time`): no
    /// model, and no miss.
    Scalar,
    /// Nothing typed it.
    Miss,
}

/// The models one site's shape resolved to, in shape order, and what missed.
#[derive(Default)]
struct SiteModels {
    hits: Vec<(NodeId, Via)>,
    unresolved: usize,
}

/// One USES edge to push: (handler, struct, form) is its key.
struct Use {
    from: NodeId,
    to: NodeId,
    form: WireForm,
    via: Via,
    /// The site's 1-based line.
    line: u32,
}

/// Resolve every json_response / json_bind wire cell of `g` and every Go
/// STRUCT's SCHEMA_FIELDS (module doc), rewriting the cells in place and
/// pushing the USES edges. Runs after the calls and refs resolve (the
/// promoted-member index is set) and changes no node, so no earlier binding
/// moves.
pub(super) fn resolve(g: &mut RepoGraph, packages: &GoPackages) -> WireStats {
    let mut stats = WireStats::default();
    let (rewrites, uses) = {
        let wire = Wire::new(g, packages);
        let mut rewrites: Vec<(usize, usize, String)> = Vec::new();
        let mut uses: Vec<Use> = Vec::new();
        let mut at: HashMap<(NodeId, NodeId, WireForm), usize> = HashMap::new();
        for (ni, node) in g.nodes.iter().enumerate() {
            for (ci, cell) in node.cells.iter().enumerate() {
                if let Some(new) = wire.site(node.id, cell, &mut stats, &mut uses, &mut at) {
                    rewrites.push((ni, ci, new));
                } else if let Some(new) = wire.nested(node.id, cell, &mut stats) {
                    rewrites.push((ni, ci, new));
                }
            }
        }
        (rewrites, uses)
    };
    for (ni, ci, payload) in rewrites {
        g.nodes[ni].cells[ci].payload = CellPayload::Json(payload);
    }
    for u in uses {
        let ev = Evidence::emitter(EMITTER)
            .rule(u.form.as_str())
            .line(u.line.saturating_sub(1));
        push_edge_with(g, u.from, u.to, edge_category::USES, ev, u.via.confidence());
        stats.uses += 1;
    }
    stats
}

/// The read-only half of [`resolve`]: one graph and its packages.
struct Wire<'a> {
    g: &'a RepoGraph,
    p: &'a GoPackages,
    /// STRUCT -> its SCHEMA_FIELDS model schema (format `go`), parsed once:
    /// a chain's last field reads its declared type text here.
    schemas: HashMap<NodeId, ModelSchema>,
}

impl<'a> Wire<'a> {
    fn new(g: &'a RepoGraph, p: &'a GoPackages) -> Self {
        let mut schemas: HashMap<NodeId, ModelSchema> = HashMap::new();
        for node in &g.nodes {
            if g.nav.kind_by_id.get(&node.id) != Some(&node_kind::STRUCT) {
                continue;
            }
            let schema = node
                .cells
                .iter()
                .filter_map(ModelSchema::from_cell)
                .find(|s| s.format == WireFormat::Go);
            if let Some(schema) = schema {
                schemas.entry(node.id).or_insert(schema);
            }
        }
        Wire { g, p, schemas }
    }

    /// The scope a node's facts are read in: its own locals (a FUNCTION /
    /// METHOD), its file's imports and package.
    fn scope_of(&self, id: NodeId) -> Option<RecvScope<'a>> {
        let module = self.p.file_of(self.g, id)?;
        let kind = self.g.nav.kind_by_id.get(&id).copied();
        let callable = kind == Some(node_kind::FUNCTION) || kind == Some(node_kind::METHOD);
        Some(RecvScope {
            func: callable.then_some(id),
            module,
            caller_dir: self.p.dir_of.get(&module)?,
            from_test: self.p.tests.contains(&module),
        })
    }

    /// `id` when it is a STRUCT: the only node a wire value models.
    fn as_struct(&self, id: NodeId) -> Option<NodeId> {
        (self.g.nav.kind_by_id.get(&id) == Some(&node_kind::STRUCT)).then_some(id)
    }

    fn qname(&self, id: NodeId) -> Option<&'a str> {
        self.g.nav.qname_by_id.get(&id).map(String::as_str)
    }

    /// A json_response / json_bind cell of `handler`, resolved: the rewritten
    /// payload when any shape resolved, `None` otherwise (and for every other
    /// cell). Its USES edges join `uses` under their (handler, struct, form)
    /// key, a stronger later site replacing a weaker earlier one.
    fn site(
        &self,
        handler: NodeId,
        cell: &glia_core::Cell,
        stats: &mut WireStats,
        uses: &mut Vec<Use>,
        at: &mut HashMap<(NodeId, NodeId, WireForm), usize>,
    ) -> Option<String> {
        let mut w = ServerWire::from_cell(cell)?;
        if !matches!(w.form, WireForm::JsonResponse | WireForm::JsonBind) {
            return None;
        }
        stats.sites += 1;
        let mut found = SiteModels::default();
        if let Some(scope) = self.scope_of(handler) {
            self.shape(scope, &mut w.shape, 1, &mut found);
        } else {
            count_open(&w.shape, 1, &mut found);
        }
        stats.unresolved += found.unresolved;
        if found.hits.is_empty() {
            return None;
        }
        stats.resolved_sites += 1;
        for &(_, via) in &found.hits {
            stats.models += 1;
            match via {
                Via::Type => stats.by_type += 1,
                Via::Call => stats.by_call += 1,
                Via::Chain => stats.by_chain += 1,
            }
        }
        // The A12.1 `type` names the value's own model, so only a top-level
        // value that resolved sets it (an envelope's keys do not).
        if let Some(model) = w.shape.model()
            && let Some(name) = model.rsplit("::").next()
        {
            w.ty = name.to_string();
        }
        // A response of unknown status is never judged, so it is no part of
        // the flow; a bind carries no status (DT.1e).
        let draws = w.form == WireForm::JsonBind || w.status.is_some();
        if draws {
            for &(to, via) in &found.hits {
                let key = (handler, to, w.form);
                match at.get(&key) {
                    Some(&i) => {
                        if via == Via::Type && uses[i].via != Via::Type {
                            uses[i].via = via;
                            uses[i].line = w.line;
                        }
                    }
                    None => {
                        at.insert(key, uses.len());
                        uses.push(Use {
                            from: handler,
                            to,
                            form: w.form,
                            via,
                            line: w.line,
                        });
                    }
                }
            }
        }
        Some(w.to_payload())
    }

    /// A STRUCT's go SCHEMA_FIELDS cell with each field's element type typed
    /// to an in-repo STRUCT: the rewritten payload when any field gained a
    /// `model`, else `None`.
    fn nested(
        &self,
        owner: NodeId,
        cell: &glia_core::Cell,
        stats: &mut WireStats,
    ) -> Option<String> {
        if cell.kind != cell_type::SCHEMA_FIELDS
            || self.g.nav.kind_by_id.get(&owner) != Some(&node_kind::STRUCT)
        {
            return None;
        }
        let mut schema = ModelSchema::from_cell(cell)?;
        if schema.format != WireFormat::Go {
            return None;
        }
        let at = RecvScope {
            func: None,
            ..self.scope_of(owner)?
        };
        let mut changed = 0usize;
        for field in &mut schema.fields {
            let (base, _) = element_type(&field.ty);
            if is_scalar(&base) {
                continue;
            }
            let Some(qname) = self.type_text(at, &base).and_then(|t| self.qname(t)) else {
                continue;
            };
            if field.model.as_deref() != Some(qname) {
                field.model = Some(qname.to_string());
                changed += 1;
            }
        }
        stats.nested += changed;
        (changed > 0).then(|| schema.to_payload())
    }

    /// Resolve `shape` (at nesting `depth`, the site's value 1) in `scope`,
    /// setting each resolved shape's `model` (and `array`) and recording what
    /// it found in `found`.
    fn shape(&self, scope: RecvScope<'a>, shape: &mut Shape, depth: usize, found: &mut SiteModels) {
        let hit = match shape {
            Shape::Named { ty, array, .. } => {
                // A text the parser already peeled keeps its one array
                // layer; one it did not is peeled here.
                let (base, peeled) = if *array {
                    (ty.clone(), false)
                } else {
                    element_type(ty)
                };
                if is_scalar(&base) {
                    return;
                }
                let hit = self.type_text(scope, &base);
                if hit.is_some() && peeled {
                    *array = true;
                }
                hit.map(|t| (t, Via::Type))
            }
            Shape::Call { chain, array, .. } => {
                let hit = self.call(scope, chain);
                if let Some((_, true)) = hit {
                    *array = true;
                }
                hit.map(|(t, _)| (t, Via::Call))
            }
            Shape::Chain { chain, array, .. } => match self.chain(scope, chain) {
                Field::Model(t, arr) => {
                    if arr {
                        *array = true;
                    }
                    Some((t, Via::Chain))
                }
                Field::Scalar => return,
                Field::Miss => None,
            },
            Shape::Object { fields, .. } => {
                if depth <= MAX_DEPTH {
                    for f in fields {
                        self.shape(scope, &mut f.shape, depth + 1, found);
                    }
                }
                return;
            }
            Shape::Union(members) => {
                for m in members {
                    self.shape(scope, m, depth, found);
                }
                return;
            }
            Shape::Open { .. } => {
                found.unresolved += 1;
                return;
            }
        };
        match hit.and_then(|(t, via)| Some((t, via, self.qname(t)?))) {
            Some((t, via, qname)) => {
                shape.set_model(qname);
                found.hits.push((t, via));
            }
            None => found.unresolved += 1,
        }
    }

    /// The STRUCT a type text names in `scope` ([`GoPackages::type_of_text`]:
    /// `pkg.T` through the file's imports, a bare `T` in its package, else
    /// repo-unique).
    fn type_text(&self, scope: RecvScope<'a>, text: &str) -> Option<NodeId> {
        let id = self
            .p
            .type_of_text(self.g, scope, text, 0, &mut Typed::default())?;
        self.as_struct(id)
    }

    /// The STRUCT a call chain's result is, and whether it is an array of it
    /// (the callee's [`NavFact::SliceResult`]).
    fn call(&self, scope: RecvScope<'a>, chain: &str) -> Option<(NodeId, bool)> {
        let segs = receiver_segments(chain)?;
        let callee = self.callee(scope, &segs)?;
        self.result_of(scope, callee)
    }

    /// The callee of a chain's last call segment:
    ///
    /// * `F()`: a local `F` (a func value) shadows it; else the def `F` of the
    ///   scope's file, else the one def `F` of its package ([`GoPackages`]'
    ///   `root_hop` Call arm).
    /// * `pkg.F()` with `pkg` an import of the scope's file (and no local of
    ///   that name): the exported def `F` of that package.
    /// * `<prefix>.m()`: the method `m` ([`GoPackages::method_on`]: declared,
    ///   an interface element, or promoted) of the type `prefix` evaluates to
    ///   ([`GoPackages::chain_type`]).
    fn callee(&self, scope: RecvScope<'a>, segs: &[Seg<'_>]) -> Option<NodeId> {
        let (&last, prefix) = segs.split_last()?;
        let Seg::Call(m) = last else {
            return None;
        };
        let local = |name: &str| {
            scope
                .func
                .and_then(|f| self.g.nav.local_types.get(&f))
                .is_some_and(|l| l.contains_key(name))
        };
        let p = self.p;
        if prefix.is_empty() {
            if local(m) {
                return None;
            }
            let dir = p.dir_of.get(&scope.module)?;
            return self
                .g
                .symbols
                .module_symbols
                .get(&scope.module)
                .and_then(|s| s.get(m).copied())
                .or_else(|| p.unique_in(self.g, dir, m, scope.module, scope.with_tests(dir)));
        }
        if let [Seg::Ident(x)] = prefix
            && *x != "self"
            && !local(x)
            && let Some(dir) = p.imported_dir(self.g, scope.module, x)
        {
            if !is_exported(m) {
                return None;
            }
            return p.unique_in(self.g, dir, m, scope.module, scope.with_tests(dir));
        }
        let (ty, _) = p.chain_type(self.g, scope, prefix, 0, &mut Typed::default())?;
        p.method_on(self.g, ty, m).map(|(method, _)| method)
    }

    /// What calling `callee` yields: a FUNCTION / METHOD's declared first
    /// result (read in the callee's own file), else its SliceResult element
    /// as an array; a STRUCT itself (a conversion `T(x)`).
    fn result_of(&self, scope: RecvScope<'a>, callee: NodeId) -> Option<(NodeId, bool)> {
        let kind = *self.g.nav.kind_by_id.get(&callee)?;
        if kind == node_kind::STRUCT {
            return Some((callee, false));
        }
        if kind != node_kind::FUNCTION && kind != node_kind::METHOD {
            return None;
        }
        let at = RecvScope {
            func: None,
            module: self.p.file_of(self.g, callee)?,
            ..scope
        };
        if let Some(ret) = self.g.nav.return_types.get(&callee) {
            return self.type_text(at, ret).map(|t| (t, false));
        }
        let elem = self
            .g
            .nav
            .nav_facts
            .get(&callee)?
            .iter()
            .find_map(|f| match f {
                NavFact::SliceResult { elem } => Some(elem.as_str()),
                _ => None,
            })?;
        self.type_text(at, elem).map(|t| (t, true))
    }

    /// What a field chain reads: the last field's declared type text in its
    /// owner's SCHEMA_FIELDS (a slice field records no `field_types` entry;
    /// a scalar field is no model), else the whole chain through
    /// [`GoPackages::chain_type`] (an unexported field, a promoted one).
    fn chain(&self, scope: RecvScope<'a>, chain: &str) -> Field {
        let Some(segs) = receiver_segments(chain) else {
            return Field::Miss;
        };
        if let Some((&Seg::Ident(field), prefix)) = segs.split_last()
            && !prefix.is_empty()
            && let Some((owner, _)) =
                self.p
                    .chain_type(self.g, scope, prefix, 0, &mut Typed::default())
            && let Some(decl) = self
                .schemas
                .get(&owner)
                .and_then(|s| s.fields.iter().find(|f| f.member() == field))
            && let Some(at) = self.scope_of(owner)
        {
            let (base, array) = element_type(&decl.ty);
            if is_scalar(&base) {
                return Field::Scalar;
            }
            if let Some(t) = self.type_text(RecvScope { func: None, ..at }, &base) {
                return Field::Model(t, array);
            }
        }
        self.p
            .chain_type(self.g, scope, &segs, 0, &mut Typed::default())
            .and_then(|(ty, _)| self.as_struct(ty))
            .map_or(Field::Miss, |t| Field::Model(t, false))
    }
}

/// A type text whose JSON kind the Go grammar knows (a predeclared type,
/// `time.Time`, `[]byte`, a map, an inline struct or interface): never an
/// in-repo model, so neither resolved nor counted.
fn is_scalar(base: &str) -> bool {
    !matches!(json_kind(WireFormat::Go, base), JsonKind::Unknown(_))
}

/// [`Wire::shape`]'s miss count for a site whose handler has no scope (no
/// file MODULE): every shape that could hold a model is unresolved.
fn count_open(shape: &Shape, depth: usize, found: &mut SiteModels) {
    match shape {
        Shape::Named { ty, .. } if is_scalar(&element_type(ty).0) => {}
        Shape::Object { fields, .. } => {
            if depth <= MAX_DEPTH {
                for f in fields {
                    count_open(&f.shape, depth + 1, found);
                }
            }
        }
        Shape::Union(members) => {
            for m in members {
                count_open(m, depth, found);
            }
        }
        _ => found.unresolved += 1,
    }
}

#[cfg(test)]
mod tests {
    //! Small Go graphs through the real parser and [`build_go_passes`] (the
    //! `build_go_with_mounts` pattern), asserting the rewritten cells, the
    //! USES edges and the stats.

    use glia_code_domain::evidence::Evidence;
    use glia_code_domain::{FileParse, GRAPH_TYPE, cell_type, edge_category, node_kind};
    use glia_core::{CellPayload, Confidence, NodeId, RepoId};

    use super::super::{GoPasses, build_go_passes};
    use crate::types::RepoGraph;

    fn repo() -> RepoId {
        RepoId::from_canonical("test://go_wire")
    }

    /// The engine's `path_to_qname`: drop `.go`, `/` -> `::`.
    fn module_qname(rel: &str) -> String {
        rel.strip_suffix(".go").unwrap_or(rel).replace('/', "::")
    }

    fn parse(rel: &str, src: &str) -> FileParse {
        glia_parser_go::parse_file(src, rel, &module_qname(rel), "example.com/app", repo())
            .expect("parse")
    }

    fn id(kind: glia_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), kind, qname)
    }

    const MODELS: &str = r#"package models

type Item struct {
	ID   string `json:"id"`
	Tags []Tag  `json:"tags"`
	Main *Tag   `json:"main,omitempty"`
	Note string `json:"note"`
}

type Tag struct {
	Name string `json:"name"`
}

type Create struct {
	Name string `json:"name" binding:"required"`
}

type Config struct {
	Defaults []Item `json:"defaults"`
	First    Item   `json:"first"`
}

type Lister interface {
	All() ([]Item, error)
	One() (Item, error)
	Each() Lister
}
"#;

    const STORE: &str = r#"package store

import "example.com/app/models"

type Store struct {
	items []models.Item
}

func (s *Store) Item(id string) (*models.Item, bool) {
	return nil, false
}

func (s *Store) Items() []*models.Item {
	return nil
}

func (s *Store) Lister() models.Lister {
	return nil
}

func Default() *models.Item {
	return nil
}
"#;

    /// The three-file app the tests vary: `models`, `store`, and `web/h.go`
    /// holding `handler`.
    fn build(handler: &str) -> GoPasses {
        let web = r#"package web

import (
	"net/http"

	"github.com/gin-gonic/gin"

	"example.com/app/models"
	"example.com/app/store"
)

type H struct {
	store *store.Store
	cfg   *models.Config
	list  models.Lister
}

{handler}
"#
        .replace("{handler}", handler);
        build_go_passes(
            repo(),
            vec![
                parse("models/m.go", MODELS),
                parse("store/store.go", STORE),
                parse("web/h.go", &web),
            ],
        )
    }

    fn method(name: &str) -> NodeId {
        id(node_kind::METHOD, &format!("web::h::H::{name}"))
    }

    fn model(name: &str) -> NodeId {
        id(node_kind::STRUCT, &format!("models::m::{name}"))
    }

    /// Every MESSAGE_TYPE payload of `node`.
    fn wire_cells(g: &RepoGraph, node: NodeId) -> Vec<String> {
        g.nodes
            .iter()
            .filter(|n| n.id == node)
            .flat_map(|n| n.cells.iter())
            .filter(|c| c.kind == cell_type::MESSAGE_TYPE)
            .map(|c| match &c.payload {
                CellPayload::Json(s) => s.clone(),
                other => panic!("MESSAGE_TYPE must be Json, got {other:?}"),
            })
            .collect()
    }

    fn schema(g: &RepoGraph, node: NodeId) -> String {
        g.nodes
            .iter()
            .filter(|n| n.id == node)
            .flat_map(|n| n.cells.iter())
            .find(|c| c.kind == cell_type::SCHEMA_FIELDS)
            .map(|c| match &c.payload {
                CellPayload::Json(s) => s.clone(),
                other => panic!("SCHEMA_FIELDS must be Json, got {other:?}"),
            })
            .expect("schema cell")
    }

    /// The USES edges as (from, to, confidence, rule, 0-based line).
    fn uses(g: &RepoGraph) -> Vec<(NodeId, NodeId, Confidence, String, Option<u32>)> {
        g.edges
            .iter()
            .filter(|e| e.category == edge_category::USES)
            .map(|e| {
                let ev = Evidence::of(e).expect("evidence");
                assert_eq!(ev.emitter, "graph:go_wire");
                (
                    e.from,
                    e.to,
                    e.confidence,
                    ev.rule.unwrap_or_default(),
                    ev.line,
                )
            })
            .collect()
    }

    #[test]
    fn call_result_struct() {
        let p = build(
            "func (h *H) Get(c *gin.Context) {\n\
             \tv, _ := h.store.Item(c.Param(\"id\"))\n\
             \tc.JSON(http.StatusOK, v)\n\
             }",
        );
        let cells = wire_cells(&p.g, method("Get"));
        assert_eq!(
            cells,
            vec![
                "{\"type\":\"Item\",\"raw\":\"v\",\"form\":\"json_response\",\"window\":\"site\",\
                 \"line\":20,\"status\":200,\"shape\":{\"call\":\"self.store.Item()\",\
                 \"model\":\"models::m::Item\"}}"
                    .to_string()
            ]
        );
        assert_eq!(
            uses(&p.g),
            vec![(
                method("Get"),
                model("Item"),
                Confidence::Medium,
                "json_response".into(),
                Some(19)
            )]
        );
        assert_eq!(
            p.wire.marker().as_deref(),
            Some(
                "[go-wire] resolved sites=1/1 models=1 (type=0 call=1 chain=0) unresolved=0 \
                 uses=1 nested=4"
            )
        );
    }

    #[test]
    fn slice_result_struct() {
        let p = build(
            "func (h *H) List(c *gin.Context) {\n\
             \tc.JSON(http.StatusOK, h.store.Items())\n\
             }",
        );
        let cells = wire_cells(&p.g, method("List"));
        assert_eq!(cells.len(), 1);
        assert!(
            cells[0].contains(
                "\"shape\":{\"array\":true,\"call\":\"self.store.Items()\",\"model\":\"models::m::Item\"}"
            ),
            "{}",
            cells[0]
        );
        assert!(cells[0].starts_with("{\"type\":\"Item\","), "{}", cells[0]);
        assert_eq!(uses(&p.g).len(), 1);
        assert_eq!(uses(&p.g)[0].1, model("Item"));
    }

    #[test]
    fn local_composite() {
        let p = build(
            "func (h *H) Make(c *gin.Context) {\n\
             \tv := &models.Item{ID: \"x\"}\n\
             \tc.JSON(http.StatusCreated, v)\n\
             }",
        );
        let cells = wire_cells(&p.g, method("Make"));
        assert!(
            cells[0].contains("\"shape\":{\"model\":\"models::m::Item\",\"type\":\"models.Item\"}"),
            "{}",
            cells[0]
        );
        assert_eq!(
            uses(&p.g),
            vec![(
                method("Make"),
                model("Item"),
                Confidence::Strong,
                "json_response".into(),
                Some(19)
            )]
        );
        assert_eq!(p.wire.by_type, 1);
    }

    #[test]
    fn envelope_value() {
        let p = build(
            "func (h *H) Page(c *gin.Context) {\n\
             \titems := h.store.Items()\n\
             \tc.JSON(http.StatusOK, gin.H{\"items\": items, \"count\": len(items), \"ok\": true})\n\
             }",
        );
        let cells = wire_cells(&p.g, method("Page"));
        assert_eq!(cells.len(), 1);
        // The envelope's own `type` stays empty: only its key models.
        assert!(cells[0].starts_with("{\"type\":\"\","), "{}", cells[0]);
        assert!(
            cells[0].contains(
                "{\"name\":\"items\",\"required\":true,\"shape\":{\"array\":true,\
                 \"call\":\"self.store.Items()\",\"model\":\"models::m::Item\"}}"
            ),
            "{}",
            cells[0]
        );
        assert_eq!(uses(&p.g).len(), 1);
        // `len(items)` is open (unresolved); the bool literal is a scalar.
        assert_eq!(p.wire.unresolved, 1);
        assert_eq!(
            (p.wire.sites, p.wire.resolved_sites, p.wire.by_call),
            (1, 1, 1)
        );
    }

    #[test]
    fn bind_type() {
        let p = build(
            "func (h *H) Post(c *gin.Context) {\n\
             \tvar req models.Create\n\
             \tif err := c.ShouldBindJSON(&req); err != nil {\n\
             \t\tc.JSON(http.StatusBadRequest, gin.H{\"error\": err.Error()})\n\
             \t\treturn\n\
             \t}\n\
             \tc.JSON(http.StatusCreated, models.Item{ID: req.Name})\n\
             }",
        );
        assert_eq!(
            uses(&p.g),
            vec![
                (
                    method("Post"),
                    model("Create"),
                    Confidence::Strong,
                    "json_bind".into(),
                    Some(19)
                ),
                (
                    method("Post"),
                    model("Item"),
                    Confidence::Strong,
                    "json_response".into(),
                    Some(23)
                ),
            ]
        );
        let cells = wire_cells(&p.g, method("Post"));
        assert!(
            cells
                .iter()
                .any(|c| c.contains("\"model\":\"models::m::Create\"")),
            "{cells:?}"
        );
    }

    #[test]
    fn chain_field() {
        let p = build(
            "func (h *H) Defaults(c *gin.Context) {\n\
             \tc.JSON(http.StatusOK, h.cfg.Defaults)\n\
             }\n\
             func (h *H) First(c *gin.Context) {\n\
             \tc.JSON(http.StatusOK, h.cfg.First)\n\
             }\n\
             func (h *H) FirstID(c *gin.Context) {\n\
             \tc.JSON(http.StatusOK, h.cfg.First.ID)\n\
             }",
        );
        // A slice field: its type text from the owner's SCHEMA_FIELDS.
        let d = wire_cells(&p.g, method("Defaults"));
        assert!(
            d[0].contains("\"shape\":{\"array\":true,\"chain\":\"self.cfg.Defaults\",\"model\":\"models::m::Item\"}"),
            "{}",
            d[0]
        );
        let f = wire_cells(&p.g, method("First"));
        assert!(
            f[0].contains("\"shape\":{\"chain\":\"self.cfg.First\",\"model\":\"models::m::Item\"}"),
            "{}",
            f[0]
        );
        let edges = uses(&p.g);
        assert_eq!(edges.len(), 2);
        assert!(
            edges
                .iter()
                .all(|e| e.1 == model("Item") && e.2 == Confidence::Medium)
        );
        // A scalar field (`ID string`) is no model and no miss.
        let id = wire_cells(&p.g, method("FirstID"));
        assert!(!id[0].contains("\"model\""), "{}", id[0]);
        assert_eq!(
            (
                p.wire.sites,
                p.wire.resolved_sites,
                p.wire.by_chain,
                p.wire.unresolved
            ),
            (3, 2, 2, 0)
        );
    }

    #[test]
    fn interface_result_unresolved() {
        let p = build(
            "func (h *H) Lister(c *gin.Context) {\n\
             \tc.JSON(http.StatusOK, h.store.Lister())\n\
             }\n\
             func (h *H) All(c *gin.Context) {\n\
             \tc.JSON(http.StatusOK, h.list.All())\n\
             }",
        );
        // An INTERFACE is no wire model: the cell stays as written.
        let l = wire_cells(&p.g, method("Lister"));
        assert!(!l[0].contains("\"model\""), "{}", l[0]);
        assert!(l[0].starts_with("{\"type\":\"\","), "{}", l[0]);
        // A call typed through an interface element's SliceResult resolves.
        let a = wire_cells(&p.g, method("All"));
        assert!(
            a[0].contains("\"shape\":{\"array\":true,\"call\":\"self.list.All()\",\"model\":\"models::m::Item\"}"),
            "{}",
            a[0]
        );
        assert_eq!(
            uses(&p.g),
            vec![(
                method("All"),
                model("Item"),
                Confidence::Medium,
                "json_response".into(),
                Some(21)
            )]
        );
        assert_eq!(
            (p.wire.sites, p.wire.resolved_sites, p.wire.unresolved),
            (2, 1, 1)
        );
    }

    #[test]
    fn unknown_status_no_edge() {
        let p = build(
            "func (h *H) Echo(c *gin.Context, code int) {\n\
             \tc.JSON(code, models.Item{})\n\
             }",
        );
        let cells = wire_cells(&p.g, method("Echo"));
        assert!(!cells[0].contains("\"status\""), "{}", cells[0]);
        // The cell resolves (data) but an unknown-status write is no edge.
        assert!(
            cells[0].contains("\"model\":\"models::m::Item\""),
            "{}",
            cells[0]
        );
        assert!(uses(&p.g).is_empty());
        assert_eq!((p.wire.resolved_sites, p.wire.uses), (1, 0));
    }

    #[test]
    fn nested_field_model() {
        let p = build("func (h *H) Noop() {}");
        let item = schema(&p.g, model("Item"));
        assert_eq!(
            item,
            "{\"fields\":[{\"field\":\"ID\",\"name\":\"id\",\"required\":true,\"type\":\"string\"},\
             {\"array\":true,\"field\":\"Tags\",\"model\":\"models::m::Tag\",\"name\":\"tags\",\
             \"required\":true,\"type\":\"[]Tag\"},\
             {\"field\":\"Main\",\"model\":\"models::m::Tag\",\"name\":\"main\",\"required\":false,\
             \"type\":\"*Tag\"},\
             {\"field\":\"Note\",\"name\":\"note\",\"required\":true,\"type\":\"string\"}],\"format\":\"go\"}"
        );
        let cfg = schema(&p.g, model("Config"));
        assert_eq!(
            cfg.matches("\"model\":\"models::m::Item\"").count(),
            2,
            "{cfg}"
        );
        // A leaf model gains nothing.
        assert!(!schema(&p.g, model("Tag")).contains("\"model\""));
        assert_eq!(p.wire.nested, 4);
        // No wire cell, but nested models: the marker still fires.
        assert_eq!(
            p.wire.marker().as_deref(),
            Some(
                "[go-wire] resolved sites=0/0 models=0 (type=0 call=0 chain=0) unresolved=0 uses=0 nested=4"
            )
        );
        assert!(uses(&p.g).is_empty());
    }

    #[test]
    fn deterministic_output() {
        let handler = "func (h *H) A(c *gin.Context) {\n\
             \tc.JSON(http.StatusOK, gin.H{\"b\": h.store.Items(), \"a\": models.Create{}, \"c\": h.cfg.First})\n\
             }\n\
             func (h *H) B(c *gin.Context) {\n\
             \tv, _ := h.store.Item(\"\")\n\
             \tc.JSON(http.StatusOK, v)\n\
             \tc.JSON(http.StatusAccepted, models.Item{})\n\
             }";
        let first = build(handler);
        let snapshot = |g: &RepoGraph| {
            let edges: Vec<String> = g.edges.iter().map(|e| format!("{e:?}")).collect();
            let cells: Vec<String> = g
                .nodes
                .iter()
                .map(|n| format!("{:?} {:?}", n.id, n.cells))
                .collect();
            (edges, cells)
        };
        let want = snapshot(&first.g);
        for _ in 0..4 {
            assert_eq!(snapshot(&build(handler).g), want);
        }
        // Edges follow the shape order; B's two sites for Item keep one
        // edge, placed at the written type (Strong).
        assert_eq!(
            uses(&first.g),
            vec![
                (
                    method("A"),
                    model("Item"),
                    Confidence::Medium,
                    "json_response".into(),
                    Some(18)
                ),
                (
                    method("A"),
                    model("Create"),
                    Confidence::Strong,
                    "json_response".into(),
                    Some(18)
                ),
                (
                    method("B"),
                    model("Item"),
                    Confidence::Strong,
                    "json_response".into(),
                    Some(23)
                ),
            ]
        );
        assert_eq!(first.wire.uses, 3);
        assert_eq!(first.wire.models, 5);
    }
}
