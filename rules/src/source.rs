//! The fact seam: [`FactSource`] (a [`GraphSource`] plus what a rule reads of a domain),
//! the [`Builtin`] trait for named native predicates, [`catalog_of`], the lazy EDB
//! loader the evaluator calls once per referenced predicate, the string builtins, and
//! [`MemSource`], a plain-vector source for tests and domain prototypes.
//!
//! A domain gives the core predicates every domain has (`node`, `edge`, `edge_conf`,
//! `cell`, `edge_cell`, `kind`, `category`, `carry_category`; `ir::Catalog::core`)
//! from four reads: a node's kind, its cells, a checkout-stable render key, and the
//! edge tier. Names come from its registries, so a node with no name (the toy reel's
//! shots are a kind and an index) still has every core row. Domain columns beyond the
//! core are attributes (`name(Node, ty)`), read in bulk per column; named native
//! predicates are builtins.
//!
//! Loading is lazy: `load_edb` reads one predicate, so a program that never names an
//! attribute never asks for its column, and a program that names only `edge` never
//! walks a cell. The evaluator narrows further with [`Restrict`]: the categories /
//! cell types every atom of a family names.
//!
//! Set semantics: edges have no id and two call sites of one callee are two edges
//! (`core::Edge::key` is not unique), so `edge/3` is a set. A row's level is the best
//! (min) tier of the edges behind it and its witness ordinal the first edge, in
//! `edges()` order, at that tier: the rule engine `why` and `implementors` use.
//! Every predicate dedups by one sort, never a hash map.
//!
//! Levels: an edge-derived row takes its edge's tier; node, cell, attribute and
//! registry rows are FACT (read at a site, R1 section 4.3). A domain that says nothing
//! about an edge's tier makes it DERIVED.

use std::fmt;

use glia_activation::algo::GraphSource;
use glia_activation::profile::{DomainTables, Registries};
use glia_core::{
    Cell, CellPayload, CellTypeId, Confidence, Edge, EdgeCategoryId, NodeId, NodeKindId,
};

use crate::analysis::EdbPred;
use crate::ir::{AttrDecl, BindReq, BuiltinDecl, BuiltinKind, Catalog, ColType};
use crate::value::{Handle, Lookup, Tier, Value, ValueRef, ValueTable};

// ============================================================================
// The traits
// ============================================================================

/// What a rule reads of a domain, on top of [`GraphSource`]'s node and edge order.
///
/// The method set is a published contract (engine's CodeFacts implements it): a
/// method added later needs a default body.
pub trait FactSource: GraphSource {
    /// The domain's registries (every kind / category / cell-type name) and carry edges.
    fn tables(&self) -> &DomainTables;

    /// The kind of node `id`; `None` for an id that is no node. Every id
    /// [`GraphSource::node_ids`] lists should have one: a listed id without a kind
    /// has no `node/2` row.
    fn node_kind(&self, id: NodeId) -> Option<NodeKindId>;

    /// The cells of node `id`; empty for an id that is no node.
    fn node_cells(&self, id: NodeId) -> &[Cell];

    /// The checkout-stable key rows sort by and witnesses tie-break on (code: the
    /// qname; toy: `shot/3`). Also called for dangling edge endpoints.
    fn render_node(&self, id: NodeId) -> String;

    /// The domain's attribute columns, each the predicate `name(Node, ty)`.
    fn attributes(&self) -> &[AttrDecl] {
        &[]
    }

    /// Bulk, in node order: every `(node, value)` of attribute `attr` (an index into
    /// [`FactSource::attributes`]). A node may have several values or none.
    fn attr_rows(&self, _attr: usize, _out: &mut Vec<(NodeId, Value)>) {}

    /// FACT / DERIVED / HEURISTIC of the edge at position `ordinal` of
    /// [`GraphSource::edges`]; a domain that says nothing is DERIVED.
    fn edge_tier(&self, _ordinal: usize, _e: &Edge) -> Tier {
        Tier::Derived
    }

    /// The domain's named native predicates (code: `in_scope/2`); none by default.
    fn builtins(&self) -> Vec<&dyn Builtin> {
        Vec::new()
    }
}

/// A named native predicate.
pub trait Builtin {
    /// Its declaration. A domain builtin's `kind` is [`BuiltinKind::Domain`]
    /// ([`catalog_of`] sets it so whatever the declaration says).
    fn decl(&self) -> BuiltinDecl;

    /// One call: `args[i]` is `Some` when column `i` is bound. Push every matching
    /// full-arity row; a fully bound call pushes one row to mean true. An `Err`
    /// message stops evaluation as ``builtin `<name>/<arity>` failed: <message>``.
    fn call(&self, args: &[Option<ValueRef<'_>>], out: &mut Vec<BuiltinRow>) -> Result<(), String>;
}

/// One row a builtin call yields, every column filled, with its evidence tier.
#[derive(Clone, Debug, PartialEq)]
pub struct BuiltinRow {
    pub values: Vec<Value>,
    pub tier: Tier,
}

// ============================================================================
// The catalog of a source
// ============================================================================

/// The source's catalog: `Catalog::core(registries, carry_edges)`, then its
/// attributes, then every domain builtin's declaration (kind set to `Domain`). A
/// declaration that collides with an earlier name is left out;
/// [`catalog_conflicts`] says which and why.
pub fn catalog_of<S: FactSource + ?Sized>(src: &S) -> Catalog {
    declare(src).catalog
}

/// One message per declaration [`catalog_of`] left out, in declaration order
/// (attributes, then builtins): a name the core, an earlier attribute or an earlier
/// builtin already owns, or a builtin whose `bind` list does not match its columns.
/// Empty for a well-formed domain, which a domain's own tests can assert.
pub fn catalog_conflicts<S: FactSource + ?Sized>(src: &S) -> Vec<String> {
    declare(src).conflicts
}

/// A source's declarations, resolved once: the catalog, the implementation behind
/// each catalog builtin, and what was left out.
pub(crate) struct Declared<'s> {
    pub(crate) catalog: Catalog,
    /// By `Catalog::builtins` index: the string builtins' static implementations
    /// and the domain's own; `None` for `reaches` / `scc` (the closure module).
    pub(crate) impls: Vec<Option<&'s dyn Builtin>>,
    pub(crate) conflicts: Vec<String>,
}

/// The owner of `name` in `cat`, for a conflict message.
fn owner_of(cat: &Catalog, n_core: usize, name: &str) -> Option<String> {
    if let Some(i) = cat.edb.iter().position(|p| p.name == name) {
        let what = if i < n_core { "the core predicate" } else { "the attribute" };
        return Some(format!("{what} `{}`", cat.edb[i].signature()));
    }
    cat.builtins.iter().find(|b| b.name == name).map(|b| format!("the builtin `{}`", b.signature()))
}

/// [`catalog_of`], with the builtin implementations and the left-out declarations.
/// Each declaration is offered to the catalog alone, so `Catalog::with_attrs` /
/// `with_builtins` decide what is kept and this only names the owner of a refusal.
pub(crate) fn declare<'s, S: FactSource + ?Sized>(src: &'s S) -> Declared<'s> {
    let t = src.tables();
    let mut cat = Catalog::core(t.registries, t.carry_edges);
    let n_core = cat.edb.len();
    let mut conflicts = Vec::new();
    for a in src.attributes() {
        if let Some(owner) = owner_of(&cat, n_core, a.name) {
            conflicts.push(format!(
                "domain attribute `{}/2` is ignored: `{}` is already {owner}",
                a.name, a.name
            ));
        }
        cat = cat.with_attrs(std::slice::from_ref(a));
    }
    let mut impls: Vec<Option<&'s dyn Builtin>> =
        cat.builtins.iter().map(|d| string_builtin(d.kind)).collect();
    for b in src.builtins() {
        let mut d = b.decl();
        d.kind = BuiltinKind::Domain;
        let sig = d.signature();
        if d.bind.len() != d.cols.len() {
            conflicts.push(format!(
                "domain builtin `{sig}` is ignored: it declares {} bind requirements for {} \
                 columns",
                d.bind.len(),
                d.cols.len()
            ));
            continue;
        }
        let before = cat.builtins.len();
        let owner = match cat.edb(&d.name) {
            Some(p) => Some(format!("`{}` is already the predicate `{}`", d.name, p.signature())),
            None => cat.builtin(&d.name, d.cols.len()).map(|e| match e.kind {
                BuiltinKind::Domain => format!("`{sig}` is already a domain builtin"),
                _ => format!("`{sig}` is already a core builtin"),
            }),
        };
        cat = cat.with_builtins(vec![d]);
        if cat.builtins.len() > before {
            impls.push(Some(b));
        } else if let Some(o) = owner {
            conflicts.push(format!("domain builtin `{sig}` is ignored: {o}"));
        }
    }
    Declared { catalog: cat, impls, conflicts }
}

// ============================================================================
// The lazy EDB loader
// ============================================================================

/// The ordinal of a row no edge derives.
pub(crate) const NO_ORDINAL: u32 = u32::MAX;

/// The rows of one EDB predicate: row-major handles, one level and one witness
/// ordinal per row. Deduplicated; ordered by handle (interning order), which no
/// output reads: relations sort their own indexes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct EdbRows {
    pub(crate) arity: usize,
    pub(crate) rows: Vec<Handle>,
    pub(crate) levels: Vec<Tier>,
    /// Edge-derived rows: the witness edge's position in `edges()`; else [`NO_ORDINAL`].
    pub(crate) ordinals: Vec<u32>,
}

/// What a load keeps: `None` keeps every category / cell type.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Restrict {
    /// For `edge`, `edge_conf` and `edge_cell`.
    pub(crate) categories: Option<Vec<EdgeCategoryId>>,
    /// For `cell` and `edge_cell`.
    pub(crate) cell_types: Option<Vec<CellTypeId>>,
}

impl Restrict {
    fn category(&self, c: EdgeCategoryId) -> bool {
        self.categories.as_ref().is_none_or(|cs| cs.contains(&c))
    }

    fn cell_type(&self, t: CellTypeId) -> bool {
        self.cell_types.as_ref().is_none_or(|ts| ts.contains(&t))
    }
}

/// Every edge's tier, by ordinal: one pass over `edges()`.
pub(crate) fn edge_tiers<S: FactSource + ?Sized>(src: &S) -> Vec<Tier> {
    src.edges().enumerate().map(|(i, e)| src.edge_tier(i, e)).collect()
}

/// An ordinal as a row column; an edge list past `u32::MAX - 1` edges saturates.
fn ordinal(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(NO_ORDINAL - 1).min(NO_ORDINAL - 1)
}

/// Rows under construction, deduplicated by [`Rows::finish`].
struct Rows {
    arity: usize,
    rows: Vec<Handle>,
    levels: Vec<Tier>,
    ordinals: Vec<u32>,
}

impl Rows {
    fn new(arity: usize) -> Self {
        Rows { arity, rows: Vec::new(), levels: Vec::new(), ordinals: Vec::new() }
    }

    /// `row` holds `arity` handles (every caller passes a literal of that length).
    fn push(&mut self, row: &[Handle], level: Tier, ord: u32) {
        self.rows.extend_from_slice(row);
        self.levels.push(level);
        self.ordinals.push(ord);
    }

    /// Sort by (row handles, level, ordinal) and keep each row's first: its best
    /// level and, at that level, its first ordinal.
    fn finish(self) -> EdbRows {
        let Rows { arity, rows, levels, ordinals } = self;
        let row = |i: usize| &rows[i * arity..(i + 1) * arity];
        let mut order: Vec<usize> = (0..levels.len()).collect();
        order.sort_unstable_by(|&a, &b| {
            row(a).cmp(row(b)).then(levels[a].cmp(&levels[b])).then(ordinals[a].cmp(&ordinals[b]))
        });
        let mut out = EdbRows { arity, ..EdbRows::default() };
        let mut last: Option<usize> = None;
        for i in order {
            if last.is_some_and(|l| row(l) == row(i)) {
                continue;
            }
            out.rows.extend_from_slice(row(i));
            out.levels.push(levels[i]);
            out.ordinals.push(ordinals[i]);
            last = Some(i);
        }
        out
    }
}

/// Interns registry names, once per id within one load. An id the registry does not
/// name renders `#<id>`.
struct Names<'r> {
    reg: &'r Registries,
    kinds: Lookup<u32, Handle>,
    categories: Lookup<u32, Handle>,
    cell_types: Lookup<u32, Handle>,
}

impl<'r> Names<'r> {
    fn new(reg: &'r Registries) -> Self {
        Names { reg, kinds: Lookup::new(), categories: Lookup::new(), cell_types: Lookup::new() }
    }

    fn intern(
        cache: &mut Lookup<u32, Handle>,
        id: u32,
        name: Option<&str>,
        values: &mut ValueTable,
    ) -> Handle {
        *cache.get_or_insert_with(id, || match name {
            Some(n) => values.intern(ValueRef::Str(n)),
            None => values.intern(ValueRef::Str(&format!("#{id}"))),
        })
    }

    fn kind(&mut self, k: NodeKindId, values: &mut ValueTable) -> Handle {
        Self::intern(&mut self.kinds, k.0, self.reg.kind_name(k), values)
    }

    fn category(&mut self, c: EdgeCategoryId, values: &mut ValueTable) -> Handle {
        Self::intern(&mut self.categories, c.0, self.reg.category_name(c), values)
    }

    fn cell_type(&mut self, t: CellTypeId, values: &mut ValueTable) -> Handle {
        Self::intern(&mut self.cell_types, t.0, self.reg.cell_name(t), values)
    }
}

/// A cell's text: Text and Json payloads; Bytes has none.
fn cell_text(c: &Cell) -> Option<&str> {
    match &c.payload {
        CellPayload::Text(s) | CellPayload::Json(s) => Some(s),
        CellPayload::Bytes(_) => None,
    }
}

/// `strong` / `medium` / `weak`: `edge_conf/4`'s fourth column.
fn confidence_name(c: Confidence) -> &'static str {
    match c {
        Confidence::Strong => "strong",
        Confidence::Medium => "medium",
        Confidence::Weak => "weak",
    }
}

/// The column of [`FactSource::attributes`] behind `Catalog::edb[edb_index]`: the
/// first attribute with that declaration's name (the one `with_attrs` kept); `None`
/// for a core predicate's index or one past the attributes.
fn attr_column<S: FactSource + ?Sized>(src: &S, edb_index: usize) -> Option<usize> {
    let t = src.tables();
    let core = Catalog::core(t.registries, t.carry_edges);
    if edb_index < core.edb.len() {
        return None;
    }
    let attrs = src.attributes();
    let cat = core.with_attrs(attrs);
    let name = cat.edb.get(edb_index)?.name.as_str();
    attrs.iter().position(|a| a.name == name)
}

/// The rows of `pred`, deduplicated (module doc). `tiers` is [`edge_tiers`]' output;
/// an edge past its end asks `edge_tier` directly, so a non-edge load may pass `&[]`.
pub(crate) fn load_edb<S: FactSource + ?Sized>(
    src: &S,
    pred: EdbPred,
    restrict: &Restrict,
    tiers: &[Tier],
    values: &mut ValueTable,
) -> EdbRows {
    let tables = src.tables();
    let mut names = Names::new(&tables.registries);
    let arity = match pred {
        EdbPred::Node | EdbPred::Attr(_) => 2,
        EdbPred::Edge | EdbPred::Cell => 3,
        EdbPred::EdgeConf => 4,
        EdbPred::EdgeCell => 5,
        EdbPred::Kind | EdbPred::Category | EdbPred::CarryCategory => 1,
    };
    let mut out = Rows::new(arity);
    let node = |values: &mut ValueTable, id: NodeId| values.intern(ValueRef::Node(id));
    match pred {
        EdbPred::Node => {
            for id in src.node_ids() {
                if let Some(k) = src.node_kind(id) {
                    let row = [node(values, id), names.kind(k, values)];
                    out.push(&row, Tier::Fact, NO_ORDINAL);
                }
            }
        }
        EdbPred::Edge | EdbPred::EdgeConf | EdbPred::EdgeCell => {
            for (i, e) in src.edges().enumerate() {
                if !restrict.category(e.category) {
                    continue;
                }
                let level = tiers.get(i).copied().unwrap_or_else(|| src.edge_tier(i, e));
                let (from, to) = (node(values, e.from), node(values, e.to));
                let cat = names.category(e.category, values);
                match pred {
                    EdbPred::Edge => out.push(&[from, to, cat], level, ordinal(i)),
                    EdbPred::EdgeConf => {
                        let conf = values.intern(ValueRef::Str(confidence_name(e.confidence)));
                        out.push(&[from, to, cat, conf], level, ordinal(i));
                    }
                    _ => {
                        for c in &e.cells {
                            let Some(text) = cell_text(c).filter(|_| restrict.cell_type(c.kind))
                            else {
                                continue;
                            };
                            let row = [
                                from,
                                to,
                                cat,
                                names.cell_type(c.kind, values),
                                values.intern(ValueRef::Str(text)),
                            ];
                            out.push(&row, level, ordinal(i));
                        }
                    }
                }
            }
        }
        EdbPred::Cell => {
            for id in src.node_ids() {
                for c in src.node_cells(id) {
                    let Some(text) = cell_text(c).filter(|_| restrict.cell_type(c.kind)) else {
                        continue;
                    };
                    let row = [
                        node(values, id),
                        names.cell_type(c.kind, values),
                        values.intern(ValueRef::Str(text)),
                    ];
                    out.push(&row, Tier::Fact, NO_ORDINAL);
                }
            }
        }
        EdbPred::Kind => {
            for (k, _) in tables.registries.node_kinds {
                out.push(&[names.kind(*k, values)], Tier::Fact, NO_ORDINAL);
            }
        }
        EdbPred::Category => {
            for (c, _) in tables.registries.edge_categories {
                out.push(&[names.category(*c, values)], Tier::Fact, NO_ORDINAL);
            }
        }
        EdbPred::CarryCategory => {
            for c in tables.carry_edges {
                out.push(&[names.category(*c, values)], Tier::Fact, NO_ORDINAL);
            }
        }
        EdbPred::Attr(d) => {
            if let Some(col) = attr_column(src, d) {
                let mut rows = Vec::new();
                src.attr_rows(col, &mut rows);
                for (id, v) in &rows {
                    let row = [node(values, *id), values.intern(v.as_ref())];
                    out.push(&row, Tier::Fact, NO_ORDINAL);
                }
            }
        }
    }
    out.finish()
}

// ============================================================================
// The string builtins
// ============================================================================

/// `starts_with(S, Prefix)`: both strings bound, FACT.
pub(crate) struct StartsWith;
/// `ends_with(S, Suffix)`: both strings bound, FACT.
pub(crate) struct EndsWith;
/// `contains(S, Part)`: both strings bound, FACT.
pub(crate) struct Contains;

/// The core declaration of a string test (`Catalog::core` declares the same).
fn string_decl(name: &str, kind: BuiltinKind) -> BuiltinDecl {
    BuiltinDecl {
        name: name.to_string(),
        cols: vec![ColType::Str, ColType::Str],
        bind: vec![BindReq::Must, BindReq::Must],
        tiered: false,
        kind,
    }
}

/// One call of a string test: an unbound argument is an error (analysis requires
/// both bound); a node or int argument is no match, as a mixed-type comparison is
/// false.
fn string_call(
    args: &[Option<ValueRef<'_>>],
    out: &mut Vec<BuiltinRow>,
    test: fn(&str, &str) -> bool,
) -> Result<(), String> {
    let [a, b] = args else {
        return Err(format!("takes 2 arguments, got {}", args.len()));
    };
    let (Some(a), Some(b)) = (a, b) else {
        let n = if a.is_none() { 1 } else { 2 };
        return Err(format!("argument {n} is unbound"));
    };
    if let (ValueRef::Str(x), ValueRef::Str(y)) = (a, b)
        && test(x, y)
    {
        out.push(BuiltinRow {
            values: vec![Value::Str((*x).to_string()), Value::Str((*y).to_string())],
            tier: Tier::Fact,
        });
    }
    Ok(())
}

impl Builtin for StartsWith {
    fn decl(&self) -> BuiltinDecl {
        string_decl("starts_with", BuiltinKind::StartsWith)
    }

    fn call(&self, args: &[Option<ValueRef<'_>>], out: &mut Vec<BuiltinRow>) -> Result<(), String> {
        string_call(args, out, |s, p| s.starts_with(p))
    }
}

impl Builtin for EndsWith {
    fn decl(&self) -> BuiltinDecl {
        string_decl("ends_with", BuiltinKind::EndsWith)
    }

    fn call(&self, args: &[Option<ValueRef<'_>>], out: &mut Vec<BuiltinRow>) -> Result<(), String> {
        string_call(args, out, |s, p| s.ends_with(p))
    }
}

impl Builtin for Contains {
    fn decl(&self) -> BuiltinDecl {
        string_decl("contains", BuiltinKind::Contains)
    }

    fn call(&self, args: &[Option<ValueRef<'_>>], out: &mut Vec<BuiltinRow>) -> Result<(), String> {
        string_call(args, out, |s, p| s.contains(p))
    }
}

/// The evaluator's implementation of a core string builtin; `None` for every other
/// kind (`reaches` / `scc` run in the closure module, a domain builtin is the
/// domain's).
pub(crate) fn string_builtin(kind: BuiltinKind) -> Option<&'static dyn Builtin> {
    match kind {
        BuiltinKind::StartsWith => Some(&StartsWith),
        BuiltinKind::EndsWith => Some(&EndsWith),
        BuiltinKind::Contains => Some(&Contains),
        BuiltinKind::Reaches | BuiltinKind::Scc | BuiltinKind::Domain => None,
    }
}

// ============================================================================
// MemSource
// ============================================================================

/// A plain-vector [`FactSource`]: nodes, attributes, cells and edges given one by one,
/// read back in the order given. For the crate's tests, engine unit tests and domain
/// prototypes; a real domain implements the trait over its own graph.
///
/// A node is an id, a kind and a render key. A repeated `node` call for one id keeps
/// its first position and replaces its kind and render. `set_attr` and `cell` on an
/// id that is no node (or an attribute index never declared) do nothing. An edge's
/// endpoints need not be nodes; a dangling id renders `#<id as 16 hex digits>`.
pub struct MemSource {
    tables: DomainTables,
    ids: Vec<NodeId>,
    kinds: Vec<NodeKindId>,
    renders: Vec<String>,
    cells: Vec<Vec<Cell>>,
    /// Node id -> its position in `ids`.
    index: Lookup<NodeId, u32>,
    attrs: Vec<AttrDecl>,
    /// `[attribute][node position]`.
    attr_values: Vec<Vec<Option<Value>>>,
    edges: Vec<Edge>,
    tiers: Vec<Tier>,
    builtins: Vec<Box<dyn Builtin>>,
}

impl MemSource {
    /// An empty source over `tables`.
    pub fn new(tables: DomainTables) -> Self {
        MemSource {
            tables,
            ids: Vec::new(),
            kinds: Vec::new(),
            renders: Vec::new(),
            cells: Vec::new(),
            index: Lookup::new(),
            attrs: Vec::new(),
            attr_values: Vec::new(),
            edges: Vec::new(),
            tiers: Vec::new(),
            builtins: Vec::new(),
        }
    }

    fn position(&self, id: NodeId) -> Option<usize> {
        self.index.get(&id).map(|i| *i as usize)
    }

    /// Add node `id` of `kind`, rendered `render`.
    pub fn node(&mut self, id: NodeId, kind: NodeKindId, render: impl Into<String>) -> &mut Self {
        let render = render.into();
        match self.position(id) {
            Some(i) => {
                self.kinds[i] = kind;
                self.renders[i] = render;
            }
            None => {
                self.index.insert(id, self.ids.len() as u32);
                self.ids.push(id);
                self.kinds.push(kind);
                self.renders.push(render);
                self.cells.push(Vec::new());
            }
        }
        self
    }

    /// Declare an attribute column; its index is the number declared before it.
    pub fn attr(&mut self, decl: AttrDecl) -> &mut Self {
        self.attrs.push(decl);
        self.attr_values.push(Vec::new());
        self
    }

    /// Set node `id`'s value of attribute `attr`, replacing an earlier one.
    pub fn set_attr(&mut self, id: NodeId, attr: usize, value: Value) -> &mut Self {
        if let (Some(i), Some(col)) = (self.position(id), self.attr_values.get_mut(attr)) {
            if col.len() <= i {
                col.resize(i + 1, None);
            }
            col[i] = Some(value);
        }
        self
    }

    /// Append `cell` to node `id`'s cells.
    pub fn cell(&mut self, id: NodeId, cell: Cell) -> &mut Self {
        if let Some(i) = self.position(id) {
            self.cells[i].push(cell);
        }
        self
    }

    /// Add an edge with no cells at `tier`.
    pub fn edge(
        &mut self,
        from: NodeId,
        to: NodeId,
        category: EdgeCategoryId,
        confidence: Confidence,
        tier: Tier,
    ) -> &mut Self {
        self.push_edge(Edge::new(from, to, category, confidence), tier)
    }

    /// Add `edge` as given (its cells are `edge_cell/5`'s rows) at `tier`.
    pub fn push_edge(&mut self, edge: Edge, tier: Tier) -> &mut Self {
        self.edges.push(edge);
        self.tiers.push(tier);
        self
    }

    /// Register a domain builtin.
    pub fn builtin(&mut self, builtin: Box<dyn Builtin>) -> &mut Self {
        self.builtins.push(builtin);
        self
    }
}

impl fmt::Debug for MemSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemSource")
            .field("graph_type", &self.tables.graph_type)
            .field("nodes", &self.ids.len())
            .field("edges", &self.edges.len())
            .field("attributes", &self.attrs.iter().map(|a| a.name).collect::<Vec<_>>())
            .field("builtins", &self.builtins.len())
            .finish()
    }
}

impl GraphSource for MemSource {
    fn node_ids(&self) -> Vec<NodeId> {
        self.ids.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

impl FactSource for MemSource {
    fn tables(&self) -> &DomainTables {
        &self.tables
    }

    fn node_kind(&self, id: NodeId) -> Option<NodeKindId> {
        self.position(id).map(|i| self.kinds[i])
    }

    fn node_cells(&self, id: NodeId) -> &[Cell] {
        self.position(id).map_or(&[], |i| self.cells[i].as_slice())
    }

    fn render_node(&self, id: NodeId) -> String {
        match self.position(id) {
            Some(i) => self.renders[i].clone(),
            None => format!("#{:016x}", id.0),
        }
    }

    fn attributes(&self) -> &[AttrDecl] {
        &self.attrs
    }

    fn attr_rows(&self, attr: usize, out: &mut Vec<(NodeId, Value)>) {
        let Some(col) = self.attr_values.get(attr) else {
            return;
        };
        for (id, v) in self.ids.iter().zip(col) {
            if let Some(v) = v {
                out.push((*id, v.clone()));
            }
        }
    }

    fn edge_tier(&self, ordinal: usize, _e: &Edge) -> Tier {
        self.tiers.get(ordinal).copied().unwrap_or(Tier::Derived)
    }

    fn builtins(&self) -> Vec<&dyn Builtin> {
        self.builtins.iter().map(|b| b.as_ref()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_activation::profile::EntryRule;
    use std::cell::{Cell as Counter, RefCell};

    const FUNCTION: NodeKindId = NodeKindId(1);
    const MODULE: NodeKindId = NodeKindId(2);
    const CALLS: EdgeCategoryId = EdgeCategoryId(1);
    const IMPORTS: EdgeCategoryId = EdgeCategoryId(2);
    const USES: EdgeCategoryId = EdgeCategoryId(3);
    const CODE: CellTypeId = CellTypeId(1);
    const POSITION: CellTypeId = CellTypeId(2);
    const EMBED: CellTypeId = CellTypeId(3);

    const TABLES: DomainTables = DomainTables {
        graph_type: "rules-test",
        registries: Registries {
            node_kinds: &[(FUNCTION, "FUNCTION"), (MODULE, "MODULE")],
            edge_categories: &[(CALLS, "CALLS"), (IMPORTS, "IMPORTS"), (USES, "USES")],
            cell_types: &[(CODE, "CODE"), (POSITION, "POSITION"), (EMBED, "EMBED")],
        },
        entry: EntryRule { kinds: &[], roles: &[], named: &[] },
        // A repeat and an unregistered category: carry_category dedups and renders `#77`.
        carry_edges: &[USES, CALLS, USES, EdgeCategoryId(77)],
        effect_sinks: &[],
        activation_weights: &[],
        activation_presets: &[],
        community_weights: &[],
        identity_rules: &[],
    };

    const A: NodeId = NodeId(0xa);
    const B: NodeId = NodeId(0xb);
    const C: NodeId = NodeId(0xc);

    fn src() -> MemSource {
        let mut g = MemSource::new(TABLES);
        g.node(A, FUNCTION, "a").node(B, FUNCTION, "b").node(C, MODULE, "c");
        g
    }

    /// Every row rendered (nodes by `render_node`), with level and ordinal, sorted.
    fn rendered<S: FactSource>(
        g: &S,
        rows: &EdbRows,
        values: &ValueTable,
    ) -> Vec<(Vec<String>, Tier, u32)> {
        let n = rows.levels.len();
        assert_eq!(rows.rows.len(), n * rows.arity);
        assert_eq!(rows.ordinals.len(), n);
        let mut out: Vec<_> = (0..n)
            .map(|i| {
                let cols = rows.rows[i * rows.arity..(i + 1) * rows.arity]
                    .iter()
                    .map(|h| match values.get(*h) {
                        ValueRef::Node(id) => g.render_node(id),
                        ValueRef::Str(s) => s.to_string(),
                        ValueRef::Int(x) => format!("{x}i"),
                    })
                    .collect();
                (cols, rows.levels[i], rows.ordinals[i])
            })
            .collect();
        out.sort();
        out
    }

    fn load<S: FactSource>(g: &S, pred: EdbPred, r: &Restrict) -> Vec<(Vec<String>, Tier, u32)> {
        let mut values = ValueTable::new();
        let tiers = edge_tiers(g);
        let rows = load_edb(g, pred, r, &tiers, &mut values);
        rendered(g, &rows, &values)
    }

    fn row(cols: &[&str], level: Tier, ord: u32) -> (Vec<String>, Tier, u32) {
        (cols.iter().map(|s| s.to_string()).collect(), level, ord)
    }

    fn text(t: CellTypeId, s: &str) -> Cell {
        Cell { kind: t, payload: CellPayload::Text(s.into()) }
    }

    /// `near(A, B)`: true for two equal ints; a test domain builtin.
    struct Near;
    impl Builtin for Near {
        fn decl(&self) -> BuiltinDecl {
            BuiltinDecl {
                name: "near".into(),
                cols: vec![ColType::Int, ColType::Int],
                bind: vec![BindReq::Must, BindReq::May],
                tiered: false,
                kind: BuiltinKind::Reaches,
            }
        }
        fn call(
            &self,
            args: &[Option<ValueRef<'_>>],
            out: &mut Vec<BuiltinRow>,
        ) -> Result<(), String> {
            if let [Some(ValueRef::Int(x)), _] = args {
                out.push(BuiltinRow {
                    values: vec![Value::Int(*x), Value::Int(*x)],
                    tier: Tier::Fact,
                });
            }
            Ok(())
        }
    }

    /// A builtin declaring `name/arity` with `bind` requirements, for the conflict test.
    struct Named(&'static str, usize, usize);
    impl Builtin for Named {
        fn decl(&self) -> BuiltinDecl {
            BuiltinDecl {
                name: self.0.into(),
                cols: vec![ColType::Str; self.1],
                bind: vec![BindReq::May; self.2],
                tiered: false,
                kind: BuiltinKind::Domain,
            }
        }
        fn call(&self, _: &[Option<ValueRef<'_>>], _: &mut Vec<BuiltinRow>) -> Result<(), String> {
            Ok(())
        }
    }

    #[test]
    fn catalog_of_lists_attrs_and_builtins() {
        let mut g = src();
        g.attr(AttrDecl { name: "index", ty: ColType::Int }).builtin(Box::new(Near));
        let core = Catalog::core(TABLES.registries, TABLES.carry_edges);
        let c = catalog_of(&g);
        let edb: Vec<String> = c.edb.iter().map(|p| p.signature()).collect();
        let core_edb: Vec<String> = core.edb.iter().map(|p| p.signature()).collect();
        assert_eq!(edb[..core_edb.len()], core_edb[..]);
        assert_eq!(edb[core_edb.len()..], ["index/2"]);
        assert_eq!(c.edb("index").map(|p| p.cols.clone()), Some(vec![ColType::Node, ColType::Int]));
        let builtins: Vec<String> = c.builtins.iter().map(|b| b.signature()).collect();
        let core_builtins: Vec<String> = core.builtins.iter().map(|b| b.signature()).collect();
        assert_eq!(builtins[..core_builtins.len()], core_builtins[..]);
        assert_eq!(builtins[core_builtins.len()..], ["near/2"]);
        // A domain builtin is a Domain builtin whatever its declaration said.
        assert_eq!(c.builtin("near", 2).map(|b| b.kind), Some(BuiltinKind::Domain));
        assert_eq!(c.registries, TABLES.registries);
        assert_eq!(c.carry, TABLES.carry_edges);
        assert!(catalog_conflicts(&g).is_empty());

        // impls: one per catalog builtin; the string tests are ours, near is the domain's.
        let d = declare(&g);
        assert_eq!(d.impls.len(), d.catalog.builtins.len());
        for (decl, imp) in d.catalog.builtins.iter().zip(&d.impls) {
            match decl.kind {
                BuiltinKind::Reaches | BuiltinKind::Scc => assert!(imp.is_none()),
                _ => assert_eq!(imp.map(|b| b.decl().name), Some(decl.name.clone())),
            }
        }
        // The string builtins declare exactly what the core catalog does.
        for k in [BuiltinKind::StartsWith, BuiltinKind::EndsWith, BuiltinKind::Contains] {
            let b = string_builtin(k).expect("a string builtin");
            assert_eq!(core.builtin(&b.decl().name, 2), Some(&b.decl()));
        }
    }

    #[test]
    fn catalog_conflicts_name_the_owner() {
        let mut g = src();
        g.attr(AttrDecl { name: "edge", ty: ColType::Int })
            .attr(AttrDecl { name: "index", ty: ColType::Int })
            .attr(AttrDecl { name: "scc", ty: ColType::Str })
            .attr(AttrDecl { name: "index", ty: ColType::Str })
            .builtin(Box::new(Named("in_scope", 2, 2)))
            .builtin(Box::new(Named("reaches", 2, 2)))
            .builtin(Box::new(Named("index", 1, 1)))
            .builtin(Box::new(Named("in_scope", 2, 2)))
            .builtin(Box::new(Named("odd", 2, 1)))
            .builtin(Box::new(Named("in_scope", 3, 3)));
        assert_eq!(
            catalog_conflicts(&g),
            [
                "domain attribute `edge/2` is ignored: `edge` is already the core predicate \
                 `edge/3`",
                "domain attribute `scc/2` is ignored: `scc` is already the builtin `scc/2`",
                "domain attribute `index/2` is ignored: `index` is already the attribute \
                 `index/2`",
                "domain builtin `reaches/2` is ignored: `reaches/2` is already a core builtin",
                "domain builtin `index/1` is ignored: `index` is already the predicate `index/2`",
                "domain builtin `in_scope/2` is ignored: `in_scope/2` is already a domain builtin",
                "domain builtin `odd/2` is ignored: it declares 1 bind requirements for 2 columns",
            ]
        );
        let d = declare(&g);
        let edb: Vec<String> = d.catalog.edb.iter().map(|p| p.signature()).collect();
        assert_eq!(edb[8..], ["index/2"]);
        let builtins: Vec<String> = d.catalog.builtins.iter().map(|b| b.signature()).collect();
        assert_eq!(builtins[8..], ["in_scope/2", "in_scope/3"]);
        // The kept implementations are the first in_scope/2 and the in_scope/3.
        let kept: Vec<usize> =
            d.impls[8..].iter().filter_map(|b| b.map(|b| b.decl().cols.len())).collect();
        assert_eq!(kept, [2, 3]);
        assert_eq!(d.impls.len(), builtins.len());
    }

    #[test]
    fn edge_rows_are_a_set_with_best_tier() {
        let mut g = src();
        g.edge(A, B, CALLS, Confidence::Strong, Tier::Heuristic)
            .edge(A, B, CALLS, Confidence::Strong, Tier::Fact)
            .edge(A, B, CALLS, Confidence::Strong, Tier::Fact);
        assert_eq!(edge_tiers(&g), [Tier::Heuristic, Tier::Fact, Tier::Fact]);
        let all = Restrict::default();
        assert_eq!(load(&g, EdbPred::Edge, &all), [row(&["a", "b", "CALLS"], Tier::Fact, 1)]);
        assert_eq!(
            load(&g, EdbPred::EdgeConf, &all),
            [row(&["a", "b", "CALLS", "strong"], Tier::Fact, 1)]
        );

        // Two confidences are two edge_conf rows but one edge row; the best tier wins
        // per row, and a later edge at the same tier does not move the witness.
        let mut h = src();
        h.edge(A, B, CALLS, Confidence::Weak, Tier::Derived)
            .edge(A, B, CALLS, Confidence::Strong, Tier::Heuristic)
            .edge(A, B, CALLS, Confidence::Weak, Tier::Derived);
        assert_eq!(load(&h, EdbPred::Edge, &all), [row(&["a", "b", "CALLS"], Tier::Derived, 0)]);
        assert_eq!(
            load(&h, EdbPred::EdgeConf, &all),
            [
                row(&["a", "b", "CALLS", "strong"], Tier::Heuristic, 1),
                row(&["a", "b", "CALLS", "weak"], Tier::Derived, 0),
            ]
        );
        // Without tiers (a non-edge caller's `&[]`) the source is asked directly.
        let mut values = ValueTable::new();
        let rows = load_edb(&h, EdbPred::Edge, &all, &[], &mut values);
        assert_eq!(rendered(&h, &rows, &values), [row(&["a", "b", "CALLS"], Tier::Derived, 0)]);
    }

    #[test]
    fn category_restriction() {
        let mut g = src();
        g.edge(A, B, CALLS, Confidence::Strong, Tier::Fact)
            .edge(A, C, IMPORTS, Confidence::Strong, Tier::Fact)
            .edge(B, C, CALLS, Confidence::Medium, Tier::Derived)
            .edge(B, A, USES, Confidence::Strong, Tier::Fact);
        let calls = Restrict { categories: Some(vec![CALLS]), cell_types: None };
        let want =
            [row(&["a", "b", "CALLS"], Tier::Fact, 0), row(&["b", "c", "CALLS"], Tier::Derived, 2)];
        assert_eq!(load(&g, EdbPred::Edge, &calls), want);
        assert_eq!(
            load(&g, EdbPred::EdgeConf, &calls),
            [
                row(&["a", "b", "CALLS", "strong"], Tier::Fact, 0),
                row(&["b", "c", "CALLS", "medium"], Tier::Derived, 2),
            ]
        );
        assert_eq!(load(&g, EdbPred::Edge, &Restrict::default()).len(), 4);
        let none = Restrict { categories: Some(Vec::new()), cell_types: None };
        assert!(load(&g, EdbPred::Edge, &none).is_empty());
    }

    #[test]
    fn cells_text_and_json_only() {
        let mut g = src();
        g.cell(A, text(CODE, "def a(): pass"))
            .cell(A, Cell { kind: POSITION, payload: CellPayload::Json("{\"line\":0}".into()) })
            .cell(A, Cell { kind: EMBED, payload: CellPayload::Bytes(vec![1, 2, 3]) })
            .cell(NodeId(0xdead), text(CODE, "ignored: no such node"));
        let all = Restrict::default();
        assert_eq!(
            load(&g, EdbPred::Cell, &all),
            [
                row(&["a", "CODE", "def a(): pass"], Tier::Fact, NO_ORDINAL),
                row(&["a", "POSITION", "{\"line\":0}"], Tier::Fact, NO_ORDINAL),
            ]
        );
        let code = Restrict { categories: None, cell_types: Some(vec![CODE]) };
        assert_eq!(
            load(&g, EdbPred::Cell, &code),
            [row(&["a", "CODE", "def a(): pass"], Tier::Fact, NO_ORDINAL)]
        );

        // edge_cell: an edge's own cells at the edge's level, both restrictions applied.
        let mut h = src();
        h.push_edge(
            Edge::new(A, B, CALLS, Confidence::Strong)
                .with_cell(text(CODE, "a()"))
                .with_cell(Cell { kind: EMBED, payload: CellPayload::Bytes(vec![0]) })
                .with_cell(Cell { kind: POSITION, payload: CellPayload::Json("{}".into()) }),
            Tier::Derived,
        )
        .push_edge(
            Edge::new(A, C, IMPORTS, Confidence::Strong).with_cell(text(CODE, "import c")),
            Tier::Fact,
        );
        assert_eq!(
            load(&h, EdbPred::EdgeCell, &all),
            [
                row(&["a", "b", "CALLS", "CODE", "a()"], Tier::Derived, 0),
                row(&["a", "b", "CALLS", "POSITION", "{}"], Tier::Derived, 0),
                row(&["a", "c", "IMPORTS", "CODE", "import c"], Tier::Fact, 1),
            ]
        );
        let both = Restrict { categories: Some(vec![CALLS]), cell_types: Some(vec![CODE]) };
        assert_eq!(
            load(&h, EdbPred::EdgeCell, &both),
            [row(&["a", "b", "CALLS", "CODE", "a()"], Tier::Derived, 0)]
        );
    }

    #[test]
    fn node_rows_name_kinds() {
        let mut g = src();
        let (dangling, odd) = (NodeId(0x1234), NodeId(0x99));
        g.node(odd, NodeKindId(99), "odd")
            // A repeated id keeps its place and takes the new kind and render.
            .node(C, FUNCTION, "c2")
            .edge(A, dangling, USES, Confidence::Strong, Tier::Heuristic);
        assert_eq!(g.node_ids(), [A, B, C, odd]);
        let all = Restrict::default();
        assert_eq!(
            load(&g, EdbPred::Node, &all),
            [
                row(&["a", "FUNCTION"], Tier::Fact, NO_ORDINAL),
                row(&["b", "FUNCTION"], Tier::Fact, NO_ORDINAL),
                row(&["c2", "FUNCTION"], Tier::Fact, NO_ORDINAL),
                row(&["odd", "#99"], Tier::Fact, NO_ORDINAL),
            ]
        );
        assert_eq!(g.render_node(dangling), "#0000000000001234");
        assert_eq!(g.node_kind(dangling), None);
        assert!(g.node_cells(dangling).is_empty());
        assert_eq!(
            load(&g, EdbPred::Edge, &all),
            [row(&["a", "#0000000000001234", "USES"], Tier::Heuristic, 0)]
        );
    }

    #[test]
    fn registry_predicates() {
        let g = src();
        let all = Restrict::default();
        let mut values = ValueTable::new();
        let carry = load_edb(&g, EdbPred::CarryCategory, &all, &[], &mut values);
        // A fresh table interns in table order, so handle order is table order.
        let names: Vec<String> = (0..carry.levels.len())
            .map(|i| match values.get(carry.rows[i]) {
                ValueRef::Str(s) => s.to_string(),
                v => panic!("{v:?}"),
            })
            .collect();
        assert_eq!(names, ["USES", "CALLS", "#77"]);
        assert!(carry.levels.iter().all(|l| *l == Tier::Fact));
        assert!(carry.ordinals.iter().all(|o| *o == NO_ORDINAL));
        assert_eq!(carry.arity, 1);
        assert_eq!(
            load(&g, EdbPred::Kind, &all),
            [row(&["FUNCTION"], Tier::Fact, NO_ORDINAL), row(&["MODULE"], Tier::Fact, NO_ORDINAL)]
        );
        let cats: Vec<Vec<String>> =
            load(&g, EdbPred::Category, &all).into_iter().map(|r| r.0).collect();
        assert_eq!(cats, [["CALLS"], ["IMPORTS"], ["USES"]]);
        // Names are interned once: the same name is the same handle across loads.
        let cats = load_edb(&g, EdbPred::Category, &all, &[], &mut values);
        assert!(cats.rows.contains(&carry.rows[0]));
    }

    /// A source counting `attr_rows` calls and cell walks, over a MemSource.
    struct Counting {
        inner: MemSource,
        cell_walks: Counter<u32>,
        attr_reads: RefCell<Vec<usize>>,
    }
    impl GraphSource for Counting {
        fn node_ids(&self) -> Vec<NodeId> {
            self.inner.node_ids()
        }
        fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
            self.inner.edges()
        }
    }
    impl FactSource for Counting {
        fn tables(&self) -> &DomainTables {
            self.inner.tables()
        }
        fn node_kind(&self, id: NodeId) -> Option<NodeKindId> {
            self.inner.node_kind(id)
        }
        fn node_cells(&self, id: NodeId) -> &[Cell] {
            self.cell_walks.set(self.cell_walks.get() + 1);
            self.inner.node_cells(id)
        }
        fn render_node(&self, id: NodeId) -> String {
            self.inner.render_node(id)
        }
        fn attributes(&self) -> &[AttrDecl] {
            self.inner.attributes()
        }
        fn attr_rows(&self, attr: usize, out: &mut Vec<(NodeId, Value)>) {
            self.attr_reads.borrow_mut().push(attr);
            self.inner.attr_rows(attr, out)
        }
        fn edge_tier(&self, ordinal: usize, e: &Edge) -> Tier {
            self.inner.edge_tier(ordinal, e)
        }
    }

    #[test]
    fn attributes_load_lazily() {
        let mut inner = src();
        // `edge` collides with the core predicate and is dropped, so the catalog's
        // index/2 is EDB 8 and file/2 EDB 9 while their columns are 1 and 2.
        inner
            .attr(AttrDecl { name: "edge", ty: ColType::Int })
            .attr(AttrDecl { name: "index", ty: ColType::Int })
            .attr(AttrDecl { name: "file", ty: ColType::Str })
            .set_attr(A, 1, Value::Int(0))
            .set_attr(C, 1, Value::Int(2))
            .set_attr(B, 2, Value::Str("b.py".into()))
            .set_attr(C, 1, Value::Int(5))
            .set_attr(NodeId(0xdead), 1, Value::Int(9))
            .set_attr(A, 7, Value::Int(9))
            .cell(A, text(CODE, "x"))
            .edge(A, B, CALLS, Confidence::Strong, Tier::Fact);
        let g =
            Counting { inner, cell_walks: Counter::new(0), attr_reads: RefCell::new(Vec::new()) };
        let cat = catalog_of(&g);
        assert_eq!(cat.edb[8].name, "index");
        assert_eq!(cat.edb[9].name, "file");
        let all = Restrict::default();

        for pred in [EdbPred::Node, EdbPred::Edge, EdbPred::EdgeConf, EdbPred::Kind] {
            load(&g, pred, &all);
        }
        assert!(g.attr_reads.borrow().clone().is_empty());
        assert_eq!(g.cell_walks.get(), 0);

        assert_eq!(
            load(&g, EdbPred::Attr(8), &all),
            [row(&["a", "0i"], Tier::Fact, NO_ORDINAL), row(&["c", "5i"], Tier::Fact, NO_ORDINAL)]
        );
        assert_eq!(g.attr_reads.borrow().clone(), [1]);
        assert_eq!(load(&g, EdbPred::Attr(9), &all), [row(&["b", "b.py"], Tier::Fact, NO_ORDINAL)]);
        assert_eq!(g.attr_reads.borrow().clone(), [1, 2]);
        // A core index or one past the attributes reads no column.
        assert!(load(&g, EdbPred::Attr(1), &all).is_empty());
        assert!(load(&g, EdbPred::Attr(10), &all).is_empty());
        assert_eq!(g.attr_reads.borrow().clone(), [1, 2]);
        assert_eq!(g.cell_walks.get(), 0);
        load(&g, EdbPred::Cell, &all);
        assert_eq!(g.cell_walks.get(), 3);
    }

    #[test]
    fn default_methods_say_derived_and_nothing_more() {
        struct Bare(Vec<Edge>);
        impl GraphSource for Bare {
            fn node_ids(&self) -> Vec<NodeId> {
                vec![A, B]
            }
            fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
                Box::new(self.0.iter())
            }
        }
        impl FactSource for Bare {
            fn tables(&self) -> &DomainTables {
                &TABLES
            }
            fn node_kind(&self, _: NodeId) -> Option<NodeKindId> {
                Some(MODULE)
            }
            fn node_cells(&self, _: NodeId) -> &[Cell] {
                &[]
            }
            fn render_node(&self, id: NodeId) -> String {
                format!("n{:x}", id.0)
            }
        }
        let g = Bare(vec![Edge::new(A, B, IMPORTS, Confidence::Strong)]);
        assert_eq!(edge_tiers(&g), [Tier::Derived]);
        assert_eq!(
            load(&g, EdbPred::Edge, &Restrict::default()),
            [row(&["na", "nb", "IMPORTS"], Tier::Derived, 0)]
        );
        let c = catalog_of(&g);
        assert_eq!((c.edb.len(), c.builtins.len()), (8, 8));
        assert!(load(&g, EdbPred::Attr(8), &Restrict::default()).is_empty());
    }

    #[test]
    fn string_builtins() {
        let s = |v: &'static str| Some(ValueRef::Str(v));
        let call = |b: &dyn Builtin, args: &[Option<ValueRef<'_>>]| {
            let mut out = Vec::new();
            b.call(args, &mut out).map(|()| out)
        };
        let yes = |a: &str, b: &str| {
            vec![BuiltinRow {
                values: vec![Value::Str(a.into()), Value::Str(b.into())],
                tier: Tier::Fact,
            }]
        };
        assert_eq!(
            call(&StartsWith, &[s("engine::check"), s("engine")]),
            Ok(yes("engine::check", "engine"))
        );
        assert_eq!(call(&StartsWith, &[s("engine::check"), s("check")]), Ok(vec![]));
        assert_eq!(
            call(&EndsWith, &[s("engine::check"), s("check")]),
            Ok(yes("engine::check", "check"))
        );
        assert_eq!(call(&EndsWith, &[s("engine::check"), s("engine")]), Ok(vec![]));
        assert_eq!(
            call(&Contains, &[s("engine::check"), s("e::c")]),
            Ok(yes("engine::check", "e::c"))
        );
        assert_eq!(call(&Contains, &[s("engine::check"), s("graph")]), Ok(vec![]));
        assert_eq!(call(&Contains, &[s("x"), s("")]), Ok(yes("x", "")));
        // A node or int is no match; an unbound argument or a wrong arity is an error.
        assert_eq!(call(&StartsWith, &[Some(ValueRef::Node(A)), s("a")]), Ok(vec![]));
        assert_eq!(call(&Contains, &[s("10"), Some(ValueRef::Int(1))]), Ok(vec![]));
        assert_eq!(call(&EndsWith, &[s("x"), None]), Err("argument 2 is unbound".to_string()));
        assert_eq!(call(&EndsWith, &[None, s("x")]), Err("argument 1 is unbound".to_string()));
        assert_eq!(call(&Contains, &[s("x")]), Err("takes 2 arguments, got 1".to_string()));
        for (k, name) in [
            (BuiltinKind::StartsWith, "starts_with"),
            (BuiltinKind::EndsWith, "ends_with"),
            (BuiltinKind::Contains, "contains"),
        ] {
            assert_eq!(string_builtin(k).map(|b| b.decl().name), Some(name.to_string()));
        }
        for k in [BuiltinKind::Reaches, BuiltinKind::Scc, BuiltinKind::Domain] {
            assert!(string_builtin(k).is_none());
        }
    }

    #[test]
    fn mem_source_reads_back_what_it_was_given() {
        let mut g = src();
        g.attr(AttrDecl { name: "index", ty: ColType::Int })
            .set_attr(B, 0, Value::Int(1))
            .edge(A, B, CALLS, Confidence::Strong, Tier::Heuristic)
            .edge(B, C, USES, Confidence::Weak, Tier::Fact)
            .builtin(Box::new(Near));
        assert_eq!(g.node_ids(), [A, B, C]);
        let keys: Vec<_> = g.edges().map(|e| (e.from, e.to, e.category, e.confidence)).collect();
        assert_eq!(keys, [(A, B, CALLS, Confidence::Strong), (B, C, USES, Confidence::Weak)]);
        let e0 = g.edges().next().cloned().expect("an edge");
        assert_eq!(g.edge_tier(0, &e0), Tier::Heuristic);
        assert_eq!(g.edge_tier(1, &e0), Tier::Fact);
        assert_eq!(g.edge_tier(2, &e0), Tier::Derived);
        assert_eq!(g.node_kind(C), Some(MODULE));
        assert_eq!(g.render_node(B), "b");
        let mut rows = Vec::new();
        g.attr_rows(0, &mut rows);
        assert_eq!(rows, [(B, Value::Int(1))]);
        rows.clear();
        g.attr_rows(3, &mut rows);
        assert!(rows.is_empty());
        assert_eq!(g.builtins().len(), 1);
        assert_eq!(
            format!("{g:?}"),
            "MemSource { graph_type: \"rules-test\", nodes: 3, edges: 2, attributes: [\"index\"], \
             builtins: 1 }"
        );
    }
}
