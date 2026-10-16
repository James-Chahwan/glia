//! The predicate prelude (DB.2c): structural predicates over the code domain,
//! written as rules ([`PRELUDE`], `prelude.dl`), with the engine relations they read.
//!
//! **Concepts, not raw categories.** A user rule names `carry`, `depends`, `effect`
//! or `unreached`, and the prelude says which categories, kinds and tables that
//! means. Every concept already exists as a table and is read from it, never
//! re-listed: `carry_category/1` is glia-rules' core relation over
//! `CODE_TABLES.carry_edges`; the engine relations below are built from check's
//! default forbid set, the profile's effect sinks, liveness and the module import
//! graph. When the profile changes, the prelude's answers change and user rules do
//! not.
//!
//! **Merge.** [`merge`] appends to a user program the prelude rules it reads,
//! directly or through other prelude rules, plus those of a prelude predicate named
//! as an output; nothing else, so an unread prelude rule costs nothing and adds no
//! output, absence mechanism or caveat. The prelude is parsed once per process
//! (source name `prelude`). A user rule whose head names a prelude predicate or an
//! engine relation is an analysis error, ``` `carry` is a prelude predicate
//! (docs/rules.md); name yours differently ```, before any other analysis runs.
//! [`Merged::sinks`] are the user program's sinks, so a caller that asks for no
//! output never gets a prelude predicate as one.
//!
//! **Engine relations** ([`RELATIONS`]), each a domain builtin of [`PreludeFacts`]
//! (the [`CodeFacts`] plus these), every column bound or free, built on first call
//! and only then:
//!
//! | relation | rows | tier | reads |
//! |---|---|---|---|
//! | `dependency_category(C)` | `check::default_forbid_categories` | FACT | nothing |
//! | `symbol_kind(K)` | CLASS, FUNCTION, METHOD (gaps' `dead_symbol` kinds) | FACT | nothing |
//! | `effect_sink(K, C, Class)` | every (kind, category) pair `CODE_TABLES.effect_sinks` names, classed by `effect_sink(kind, category)`: the first match | FACT | nothing |
//! | `live(N)` | `entrypoint_reachable(merged)`, in node order | DERIVED (a computed walk) | the carry categories |
//! | `module_import(A, B)` | `cycles::module_import_graph`'s MODULE -> MODULE edges | `why::tier_of` of the import behind each | IMPORTS |
//!
//! A relation's `reads` is what a negation of it reads (Q5): `!live(n)` carries the
//! coverage caveats of every carry category, and an empty answer through it names
//! them as its mechanisms ([`relation_reads`]).
//!
//! **Marker**, once per evaluation: `[rules] prelude relations=<n>
//! (dependency_category=<0|1> symbol_kind=<0|1> effect_sink=<0|1> live=<0|1>
//! module_import=<0|1>)`, `n` the relations built ([`PreludeFacts::marker`]).
//!
//! Structure only (SECURITY.md): no prelude predicate or engine relation names a
//! security concept, reads a value flow or reads source text; the unit test
//! `prelude_is_structural_only` enforces it.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use glia_activation::algo::GraphSource;
use glia_activation::profile::DomainTables;
use glia_code_domain::{edge_category, node_kind};
use glia_core::{Cell, Edge, EdgeCategoryId, NodeId, NodeKindId};
use glia_graph::MergedGraph;
use glia_rules::diag::{Diagnostic, RulesError};
use glia_rules::ir::{AttrDecl, BindReq, BuiltinDecl, BuiltinKind, ColType, Literal, Program};
use glia_rules::parser::parse_program;
use glia_rules::source::{Builtin, BuiltinRow, FactSource};
use glia_rules::value::{Tier, Value, ValueRef};

use super::facts::CodeFacts;
use crate::answers::entrypoint_reachable;
use crate::check::default_forbid_categories;
use crate::cycles::module_import_graph;
use crate::gaps::DEAD_SYMBOL_KINDS;
use crate::profile::CODE_PROFILE;
use crate::why;

/// The prelude's rule text (`engine/src/rules/prelude.dl`).
pub const PRELUDE: &str = include_str!("prelude.dl");

/// The source name the prelude's spans carry.
const PRELUDE_SOURCE: &str = "prelude";

/// Every predicate [`PRELUDE`] defines, as `(name, arity)`, in first-definition
/// order: the names a user rule may not define (docs/rules.md, `## The prelude`).
pub fn prelude_predicates() -> Vec<(&'static str, usize)> {
    let Ok(p) = parsed() else {
        return Vec::new();
    };
    let mut out: Vec<(&'static str, usize)> = Vec::new();
    for r in &p.rules {
        let sig = (r.head.pred.as_str(), r.head.arity());
        if !out.contains(&sig) {
            out.push(sig);
        }
    }
    out
}

/// The prelude, parsed once.
fn parsed() -> Result<&'static Program, RulesError> {
    static PARSED: OnceLock<Result<Program, RulesError>> = OnceLock::new();
    match PARSED.get_or_init(|| parse_program(PRELUDE_SOURCE, PRELUDE)) {
        Ok(p) => Ok(p),
        Err(e) => Err(e.clone()),
    }
}

// ============================================================================
// The engine relations
// ============================================================================

/// Which engine relation a [`RelationSpec`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rel {
    DependencyCategory,
    SymbolKind,
    EffectSink,
    Live,
    ModuleImport,
}

/// The edge categories a relation's rows are computed from.
#[derive(Clone, Copy, Debug)]
enum Reads {
    Nothing,
    /// The code profile's carry edges.
    Carry,
    Named(&'static [EdgeCategoryId]),
}

/// One engine relation: a domain builtin whose every column may be bound or free.
#[derive(Debug)]
pub(crate) struct RelationSpec {
    pub(crate) rel: Rel,
    pub(crate) name: &'static str,
    pub(crate) cols: &'static [ColType],
    /// Its rows carry their own tier; an untiered relation's rows read FACT.
    pub(crate) tiered: bool,
    reads: Reads,
}

impl RelationSpec {
    /// The categories its rows are computed from, by name, in registry order: what
    /// a negation of it reads (Q5) and an empty answer through it names.
    pub(crate) fn reads(&self) -> Vec<&'static str> {
        let ids: &[EdgeCategoryId] = match self.reads {
            Reads::Nothing => &[],
            Reads::Carry => CODE_PROFILE.tables.carry_edges,
            Reads::Named(c) => c,
        };
        CODE_PROFILE
            .tables
            .registries
            .edge_categories
            .iter()
            .filter(|(id, _)| ids.contains(id))
            .map(|(_, n)| *n)
            .collect()
    }

    fn decl(&self) -> BuiltinDecl {
        BuiltinDecl {
            name: self.name.to_string(),
            cols: self.cols.to_vec(),
            bind: vec![BindReq::May; self.cols.len()],
            tiered: self.tiered,
            kind: BuiltinKind::Domain,
        }
    }
}

/// The engine relations, in marker order (module doc).
pub(crate) const RELATIONS: [RelationSpec; 5] = [
    RelationSpec {
        rel: Rel::DependencyCategory,
        name: "dependency_category",
        cols: &[ColType::Category],
        tiered: false,
        reads: Reads::Nothing,
    },
    RelationSpec {
        rel: Rel::SymbolKind,
        name: "symbol_kind",
        cols: &[ColType::Kind],
        tiered: false,
        reads: Reads::Nothing,
    },
    RelationSpec {
        rel: Rel::EffectSink,
        name: "effect_sink",
        cols: &[ColType::Kind, ColType::Category, ColType::Str],
        tiered: false,
        reads: Reads::Nothing,
    },
    RelationSpec {
        rel: Rel::Live,
        name: "live",
        cols: &[ColType::Node],
        tiered: true,
        reads: Reads::Carry,
    },
    RelationSpec {
        rel: Rel::ModuleImport,
        name: "module_import",
        cols: &[ColType::Node, ColType::Node],
        tiered: true,
        reads: Reads::Named(&[edge_category::IMPORTS]),
    },
];

/// The engine relation `name/arity`.
pub(crate) fn relation(name: &str, arity: usize) -> Option<&'static RelationSpec> {
    RELATIONS
        .iter()
        .find(|r| r.name == name && r.cols.len() == arity)
}

/// Is `name` an engine relation's, at any arity?
fn is_relation_name(name: &str) -> bool {
    RELATIONS.iter().any(|r| r.name == name)
}

/// A row read FACT.
fn fact(values: Vec<Value>) -> BuiltinRow {
    BuiltinRow {
        values,
        tier: Tier::Fact,
    }
}

fn text(s: &str) -> Value {
    Value::Str(s.to_string())
}

/// `why`'s word as a [`Tier`]; anything else is DERIVED, `why`'s own fallback.
fn tier_named(word: &str) -> Tier {
    Tier::ALL
        .into_iter()
        .find(|t| t.name() == word)
        .unwrap_or(Tier::Derived)
}

/// `effect_sink/3`: every (kind, category) pair a sink names, once, in table order,
/// with the class the first matching sink gives it.
fn effect_sink_rows(tables: &DomainTables) -> Vec<BuiltinRow> {
    let mut seen: Vec<(NodeKindId, EdgeCategoryId)> = Vec::new();
    let mut out = Vec::new();
    for sink in tables.effect_sinks {
        for &k in sink.kinds {
            for &c in sink.via {
                if seen.contains(&(k, c)) {
                    continue;
                }
                seen.push((k, c));
                if let Some((_, first)) = tables.effect_sink(k, c) {
                    out.push(fact(vec![
                        text(node_kind::name(k)),
                        text(edge_category::name(c)),
                        text(first.class),
                    ]));
                }
            }
        }
    }
    out
}

/// One relation's rows with a per-column index, each built on its first bound
/// lookup (looked up only, never iterated).
struct Table {
    rows: Vec<BuiltinRow>,
    index: Vec<OnceCell<HashMap<Value, Vec<usize>>>>,
}

impl Table {
    fn new(rows: Vec<BuiltinRow>, arity: usize) -> Self {
        Table {
            rows,
            index: (0..arity).map(|_| OnceCell::new()).collect(),
        }
    }

    /// Every row agreeing with each bound argument.
    fn select(&self, args: &[Option<ValueRef<'_>>], out: &mut Vec<BuiltinRow>) {
        let agrees = |r: &BuiltinRow| {
            args.iter()
                .zip(&r.values)
                .all(|(a, v)| a.is_none_or(|a| v.as_ref() == a))
        };
        let bound = args.iter().enumerate().find_map(|(c, a)| a.map(|a| (c, a)));
        let Some((c, key)) = bound else {
            out.extend(self.rows.iter().cloned());
            return;
        };
        let Some(slot) = self.index.get(c) else {
            return;
        };
        let index = slot.get_or_init(|| {
            let mut m: HashMap<Value, Vec<usize>> = HashMap::new();
            for (i, r) in self.rows.iter().enumerate() {
                if let Some(v) = r.values.get(c) {
                    m.entry(v.clone()).or_default().push(i);
                }
            }
            m
        });
        if let Some(hits) = index.get(&key.to_owned()) {
            out.extend(
                hits.iter()
                    .filter_map(|&i| self.rows.get(i))
                    .filter(|r| agrees(r))
                    .cloned(),
            );
        }
    }
}

/// One engine relation over one graph, built on its first call.
struct EngineRelation<'f, 'a> {
    spec: &'static RelationSpec,
    facts: &'f CodeFacts<'a>,
    merged: &'a MergedGraph,
    table: OnceCell<Table>,
}

impl EngineRelation<'_, '_> {
    fn build(&self) -> Vec<BuiltinRow> {
        let tables = &CODE_PROFILE.tables;
        match self.spec.rel {
            Rel::DependencyCategory => default_forbid_categories()
                .into_iter()
                .map(|c| fact(vec![text(edge_category::name(c))]))
                .collect(),
            // The kinds gaps' `dead_symbol` row reads, from the one list.
            Rel::SymbolKind => DEAD_SYMBOL_KINDS
                .iter()
                .map(|k| fact(vec![text(node_kind::name(*k))]))
                .collect(),
            Rel::EffectSink => effect_sink_rows(tables),
            Rel::Live => {
                let live = entrypoint_reachable(self.merged);
                self.facts
                    .node_ids()
                    .into_iter()
                    .filter(|id| live.contains(id))
                    .map(|id| BuiltinRow {
                        values: vec![Value::Node(id)],
                        tier: Tier::Derived,
                    })
                    .collect()
            }
            Rel::ModuleImport => {
                let imports = module_import_graph(self.merged);
                imports
                    .edges()
                    .map(|e| BuiltinRow {
                        values: vec![Value::Node(e.from), Value::Node(e.to)],
                        tier: tier_named(why::tier_of(imports.evidence(e.from, e.to), e).0),
                    })
                    .collect()
            }
        }
    }

    fn built(&self) -> bool {
        self.table.get().is_some()
    }
}

impl Builtin for EngineRelation<'_, '_> {
    fn decl(&self) -> BuiltinDecl {
        self.spec.decl()
    }

    fn call(&self, args: &[Option<ValueRef<'_>>], out: &mut Vec<BuiltinRow>) -> Result<(), String> {
        let arity = self.spec.cols.len();
        if args.len() != arity {
            return Err(format!("takes {arity} arguments, got {}", args.len()));
        }
        self.table
            .get_or_init(|| Table::new(self.build(), arity))
            .select(args, out);
        Ok(())
    }
}

/// The [`CodeFacts`] with the engine relations as domain builtins: the source every
/// query, validation and rule invariant evaluates over (module doc).
pub(crate) struct PreludeFacts<'f, 'a> {
    facts: &'f CodeFacts<'a>,
    relations: Vec<EngineRelation<'f, 'a>>,
}

impl<'f, 'a> PreludeFacts<'f, 'a> {
    /// `facts` (which must be facts over `merged`) plus the engine relations over
    /// `merged`, none built yet.
    pub(crate) fn new(merged: &'a MergedGraph, facts: &'f CodeFacts<'a>) -> Self {
        PreludeFacts {
            facts,
            relations: RELATIONS
                .iter()
                .map(|spec| EngineRelation {
                    spec,
                    facts,
                    merged,
                    table: OnceCell::new(),
                })
                .collect(),
        }
    }

    /// The code facts the engine relations sit beside.
    pub(crate) fn facts(&self) -> &'f CodeFacts<'a> {
        self.facts
    }

    /// Per engine relation, in [`RELATIONS`] order: its name and whether it was built.
    pub(crate) fn built(&self) -> Vec<(&'static str, bool)> {
        self.relations
            .iter()
            .map(|r| (r.spec.name, r.built()))
            .collect()
    }

    /// The `[rules] prelude relations=...` marker line (module doc).
    pub(crate) fn marker(&self) -> String {
        marker_line(&self.built())
    }
}

/// `[rules] prelude relations=<n> (<name>=<0|1> ...)` for `built`.
pub(crate) fn marker_line(built: &[(&'static str, bool)]) -> String {
    let parts: Vec<String> = built
        .iter()
        .map(|(n, b)| format!("{n}={}", u8::from(*b)))
        .collect();
    format!(
        "[rules] prelude relations={} ({})",
        built.iter().filter(|(_, b)| *b).count(),
        parts.join(" ")
    )
}

impl GraphSource for PreludeFacts<'_, '_> {
    fn node_ids(&self) -> Vec<NodeId> {
        self.facts.node_ids()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        self.facts.edges()
    }
}

impl FactSource for PreludeFacts<'_, '_> {
    fn tables(&self) -> &DomainTables {
        self.facts.tables()
    }

    fn node_kind(&self, id: NodeId) -> Option<NodeKindId> {
        self.facts.node_kind(id)
    }

    fn node_cells(&self, id: NodeId) -> &[Cell] {
        self.facts.node_cells(id)
    }

    fn render_node(&self, id: NodeId) -> String {
        self.facts.render_node(id)
    }

    fn attributes(&self) -> &[AttrDecl] {
        self.facts.attributes()
    }

    fn attr_rows(&self, attr: usize, out: &mut Vec<(NodeId, Value)>) {
        self.facts.attr_rows(attr, out);
    }

    fn edge_tier(&self, ordinal: usize, e: &Edge) -> Tier {
        self.facts.edge_tier(ordinal, e)
    }

    fn builtins(&self) -> Vec<&dyn Builtin> {
        let mut out = self.facts.builtins();
        out.extend(self.relations.iter().map(|r| r as &dyn Builtin));
        out
    }
}

// ============================================================================
// The merge
// ============================================================================

/// A user program with the prelude rules it reads appended.
pub(crate) struct Merged {
    pub(crate) program: Program,
    /// The user program's sinks as `name/arity`, in first-definition order: every
    /// predicate it defines that no rule of ANOTHER predicate reads (glia-rules'
    /// default outputs, computed before the prelude joins).
    pub(crate) sinks: Vec<String>,
}

/// Append to `user` the prelude rules it reads, and those of every prelude
/// predicate `outputs` names (`name` or `name/arity`); refuse a user rule that
/// defines a prelude predicate or an engine relation (module doc).
pub(crate) fn merge(user: Program, outputs: &[String]) -> Result<Merged, RulesError> {
    let prelude = parsed()?;
    let defines = |name: &str| prelude.rules.iter().any(|r| r.head.pred == name);
    let mut errors = Vec::new();
    for r in &user.rules {
        let name = r.head.pred.as_str();
        let what = if defines(name) {
            "predicate"
        } else if is_relation_name(name) {
            "relation"
        } else {
            continue;
        };
        errors.push(Diagnostic::error(
            r.head.span,
            user.source_name(r.head.span.source),
            Some(&r.head.signature()),
            format!("`{name}` is a prelude {what} (docs/rules.md); name yours differently"),
        ));
    }
    if !errors.is_empty() {
        return Err(RulesError::Invalid(errors));
    }
    let sinks = sinks_of(&user);
    // The prelude predicates read: from the user's bodies and outputs, then through
    // the prelude's own rules.
    let mut wanted = Wanted {
        prelude,
        names: Vec::new(),
        stack: Vec::new(),
    };
    for r in &user.rules {
        wanted.body(&r.body);
    }
    for o in outputs {
        wanted.add(o.rsplit_once('/').map_or(o.as_str(), |(n, _)| n));
    }
    while let Some(p) = wanted.stack.pop() {
        for r in prelude.rules.iter().filter(|r| r.head.pred == p) {
            wanted.body(&r.body);
        }
    }
    let wanted = wanted.names;
    let mut program = user;
    if !wanted.is_empty() {
        let mut part = prelude.clone();
        part.rules
            .retain(|r| wanted.contains(&r.head.pred.as_str()));
        program.append(part);
    }
    Ok(Merged { program, sinks })
}

/// The prelude predicates a merge takes, each once, with those still to follow.
struct Wanted {
    prelude: &'static Program,
    names: Vec<&'static str>,
    stack: Vec<&'static str>,
}

impl Wanted {
    /// Take `name` when the prelude defines it.
    fn add(&mut self, name: &str) {
        if let Some(r) = self.prelude.rules.iter().find(|r| r.head.pred == name) {
            let n = r.head.pred.as_str();
            if !self.names.contains(&n) {
                self.names.push(n);
                self.stack.push(n);
            }
        }
    }

    /// Take every prelude predicate `body` reads, positive or negated.
    fn body(&mut self, body: &[Literal]) {
        for lit in body {
            if let Literal::Pos(a) | Literal::Neg(a) = lit {
                self.add(&a.pred);
            }
        }
    }
}

/// Every predicate `p` defines that no rule of another predicate reads, as
/// `name/arity`, in first-definition order.
fn sinks_of(p: &Program) -> Vec<String> {
    let mut read: Vec<String> = Vec::new();
    for r in &p.rules {
        let head = r.head.signature();
        for lit in &r.body {
            if let Literal::Pos(a) | Literal::Neg(a) = lit {
                let sig = a.signature();
                if sig != head && !read.contains(&sig) {
                    read.push(sig);
                }
            }
        }
    }
    let mut out: Vec<String> = Vec::new();
    for r in &p.rules {
        let sig = r.head.signature();
        if !read.contains(&sig) && !out.contains(&sig) {
            out.push(sig);
        }
    }
    out
}

/// The categories the engine relations reachable from `sig` (`name/arity`) read,
/// through `program`'s rules, positive or negated; a relation's own when `sig` is
/// one. The half of a negation's cone (Q5) glia-rules cannot see: a domain
/// builtin's absence reads no category it knows.
pub(crate) fn relation_reads(program: &Program, sig: &str) -> Vec<&'static str> {
    let (name, arity) = match sig.rsplit_once('/') {
        Some((n, a)) => (n, a.parse().unwrap_or(0)),
        None => (sig, 0),
    };
    let mut out: Vec<&'static str> = Vec::new();
    let add = |rel: &RelationSpec, out: &mut Vec<&'static str>| {
        for c in rel.reads() {
            if !out.contains(&c) {
                out.push(c);
            }
        }
    };
    if let Some(rel) = relation(name, arity) {
        add(rel, &mut out);
        return out;
    }
    let mut seen: Vec<(&str, usize)> = vec![(name, arity)];
    let mut stack: Vec<(&str, usize)> = vec![(name, arity)];
    while let Some((p, n)) = stack.pop() {
        for r in program
            .rules
            .iter()
            .filter(|r| r.head.pred == p && r.head.arity() == n)
        {
            for lit in &r.body {
                let (Literal::Pos(a) | Literal::Neg(a)) = lit else {
                    continue;
                };
                let next = (a.pred.as_str(), a.args.len());
                if let Some(rel) = relation(next.0, next.1) {
                    add(rel, &mut out);
                } else if !seen.contains(&next) {
                    seen.push(next);
                    stack.push(next);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::cell_type;
    use glia_rules::analysis::analyse;
    use glia_rules::ir::Term;
    use glia_rules::source::{catalog_conflicts, catalog_of};

    use crate::rules::query::{Run, run_program};

    /// Substrings no prelude predicate or engine relation name may hold: the
    /// security concepts SECURITY.md keeps out of the query surface.
    const DENY: [&str; 16] = [
        "auth",
        "login",
        "passw",
        "secret",
        "credential",
        "token",
        "permission",
        "privilege",
        "taint",
        "sanitiz",
        "vuln",
        "cve",
        "exploit",
        "bypass",
        "attack",
        "unauthenticated",
    ];

    /// Cell types whose text is source, docs, schemas or config values.
    const TEXT_CELLS: [glia_core::CellTypeId; 4] = [
        cell_type::CODE,
        cell_type::DOC,
        cell_type::SCHEMA_FIELDS,
        cell_type::ENV,
    ];

    fn empty() -> MergedGraph {
        MergedGraph::new(Vec::new())
    }

    #[test]
    fn prelude_parses_and_analyses_clean() {
        let p = parsed().expect("the prelude parses");
        let g = empty();
        let facts = CodeFacts::new(&g);
        let src = PreludeFacts::new(&g, &facts);
        assert!(
            catalog_conflicts(&src).is_empty(),
            "{:?}",
            catalog_conflicts(&src)
        );
        let an = analyse(p, &catalog_of(&src)).expect("the prelude analyses");
        assert!(an.warnings.is_empty(), "{:?}", an.warnings);
        // One rule per concept, a comment line above each.
        let lines: Vec<&str> = PRELUDE.lines().collect();
        let mut rules = 0;
        for (i, l) in lines.iter().enumerate() {
            if l.trim().is_empty() || l.starts_with("//") {
                continue;
            }
            rules += 1;
            assert!(
                i > 0 && lines[i - 1].starts_with("//"),
                "no comment above `{l}`"
            );
        }
        assert_eq!(rules, p.rules.len());
        assert_eq!(prelude_predicates().len(), p.rules.len(), "one rule each");
    }

    #[test]
    fn prelude_is_structural_only() {
        let mut names: Vec<&str> = prelude_predicates().iter().map(|(n, _)| *n).collect();
        names.extend(RELATIONS.iter().map(|r| r.name));
        for name in &names {
            let lower = name.to_ascii_lowercase();
            for deny in DENY {
                assert!(!lower.contains(deny), "`{name}` names `{deny}`");
            }
            // DB.7b: the GQL lowering owns `result` and the `gql_` names.
            assert!(*name != "result" && !name.starts_with("gql_"), "{name}");
        }
        // No prelude rule reads a text cell, or a cell type it does not name.
        let p = parsed().expect("parses");
        for r in &p.rules {
            for lit in &r.body {
                let (Literal::Pos(a) | Literal::Neg(a)) = lit else {
                    continue;
                };
                let ty = match (a.pred.as_str(), a.args.len()) {
                    ("cell", 3) => a.args.get(1),
                    ("edge_cell", 5) => a.args.get(3),
                    _ => continue,
                };
                let Some(Term::Str { value, .. }) = ty else {
                    panic!("`{r}` reads a cell type it does not name");
                };
                assert!(
                    !TEXT_CELLS.iter().any(|t| cell_type::name(*t) == value),
                    "`{r}` reads {value} text"
                );
            }
        }
        // No engine relation yields text: every string it can produce is a
        // registry name or an effect-sink class, and live / module_import hold
        // nodes only.
        let g = empty();
        let facts = CodeFacts::new(&g);
        let src = PreludeFacts::new(&g, &facts);
        let tables = &CODE_PROFILE.tables;
        let known = |s: &str| {
            tables.registries.node_kinds.iter().any(|(_, n)| *n == s)
                || tables
                    .registries
                    .edge_categories
                    .iter()
                    .any(|(_, n)| *n == s)
                || tables.effect_sinks.iter().any(|e| e.class == s)
        };
        for rel in &src.relations {
            let mut rows = Vec::new();
            rel.call(&vec![None; rel.spec.cols.len()], &mut rows)
                .expect("a free call");
            for v in rows.iter().flat_map(|r| &r.values) {
                if let Value::Str(s) = v {
                    assert!(known(s), "{} yields `{s}`", rel.spec.name);
                }
            }
            let strings = rel.spec.cols.iter().filter(|c| **c == ColType::Str).count();
            let allowed = usize::from(rel.spec.rel == Rel::EffectSink);
            assert_eq!(strings, allowed, "{} has a free-text column", rel.spec.name);
        }
    }

    #[test]
    fn static_relations_read_the_tables() {
        let g = empty();
        let facts = CodeFacts::new(&g);
        let src = PreludeFacts::new(&g, &facts);
        let rows_of = |name: &str, args: &[Option<ValueRef<'_>>]| -> Vec<Vec<Value>> {
            let rel = src
                .relations
                .iter()
                .find(|r| r.spec.name == name)
                .expect("a relation");
            let mut out = Vec::new();
            rel.call(args, &mut out).expect("a call");
            out.into_iter().map(|r| r.values).collect()
        };
        let deps: Vec<Vec<Value>> = default_forbid_categories()
            .into_iter()
            .map(|c| vec![text(edge_category::name(c))])
            .collect();
        assert_eq!(rows_of("dependency_category", &[None]), deps);
        assert_eq!(
            rows_of("symbol_kind", &[None]),
            [
                vec![text("CLASS")],
                vec![text("FUNCTION")],
                vec![text("METHOD")]
            ]
        );
        // The first matching sink classifies, bound or free.
        let tables = &CODE_PROFILE.tables;
        for row in rows_of("effect_sink", &[None, None, None]) {
            let [Value::Str(k), Value::Str(c), Value::Str(class)] = row.as_slice() else {
                panic!("three strings: {row:?}");
            };
            let kind = node_kind::ALL.iter().find(|(_, n)| n == k).map(|(i, _)| *i);
            let cat = edge_category::ALL
                .iter()
                .find(|(_, n)| n == c)
                .map(|(i, _)| *i);
            let first = tables.effect_sink(kind.expect("a kind"), cat.expect("a category"));
            assert_eq!(first.map(|(_, s)| s.class), Some(class.as_str()));
        }
        assert_eq!(
            rows_of(
                "effect_sink",
                &[
                    Some(ValueRef::Str("ENDPOINT")),
                    Some(ValueRef::Str("CALLS")),
                    None
                ]
            ),
            [vec![text("ENDPOINT"), text("CALLS"), text("http_call")]]
        );
        assert!(
            rows_of(
                "effect_sink",
                &[Some(ValueRef::Str("FUNCTION")), None, None]
            )
            .is_empty()
        );
        // Only the relations called were built.
        let on: Vec<&str> = src
            .built()
            .iter()
            .filter(|(_, b)| *b)
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(on, ["dependency_category", "symbol_kind", "effect_sink"]);
        assert_eq!(
            relation("live", 1)
                .map(RelationSpec::reads)
                .map(|r| r.len()),
            Some(tables.carry_edges.len())
        );
        assert_eq!(
            relation("module_import", 2).map(RelationSpec::reads),
            Some(vec!["IMPORTS"])
        );
        assert!(relation("live", 2).is_none());
    }

    /// A tiny python repo: `main()` calls `used()`; `dead()` calls nothing.
    fn repo() -> (tempfile::TempDir, MergedGraph) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = "def used():\n    return 1\n\n\ndef dead():\n    return 2\n\n\ndef main():\n    return used()\n";
        std::fs::write(tmp.path().join("app.py"), src).expect("write");
        let r = crate::generate_one(tmp.path().to_str().expect("utf-8")).expect("generate_one");
        (tmp, r.merged)
    }

    fn evaluate(merged: &MergedGraph, text: &str) -> (Vec<String>, Vec<(&'static str, bool)>) {
        let facts = CodeFacts::new(merged);
        let src = PreludeFacts::new(merged, &facts);
        let user = parse_program("<test>", text).expect("parses");
        let m = merge(user, &[]).expect("merges");
        let run = Run {
            outputs: &m.sinks,
            explain: false,
            budget: None,
            limit: None,
        };
        let ev = run_program(&src, &m.program, &run).expect("evaluates");
        let rows = ev
            .outputs
            .iter()
            .flat_map(|o| &o.rows)
            .map(|r| match r.values.first() {
                Some(Value::Node(id)) => facts.render_node(*id),
                other => format!("{other:?}"),
            })
            .collect();
        (rows, src.built())
    }

    #[test]
    fn unused_relations_are_not_built() {
        let (_tmp, merged) = repo();
        let (rows, built) = evaluate(&merged, "x(a, b) :- edge(a, b, \"CALLS\").");
        assert_eq!(rows, ["app::main"]);
        assert_eq!(built.iter().filter(|(_, b)| *b).count(), 0, "{built:?}");
        assert_eq!(
            marker_line(&built),
            "[rules] prelude relations=0 (dependency_category=0 symbol_kind=0 effect_sink=0 live=0 module_import=0)"
        );
        // A prelude rule over edges alone builds no relation either.
        let (_, built) = evaluate(&merged, "x(a, b) :- calls(a, b).");
        assert!(built.iter().all(|(_, b)| !b), "{built:?}");
        // `unreached` builds liveness and the symbol kinds, nothing else.
        let (rows, built) = evaluate(&merged, "x(n) :- unreached(n).");
        assert_eq!(rows, ["app::dead"]);
        let on: Vec<&str> = built.iter().filter(|(_, b)| *b).map(|(n, _)| *n).collect();
        assert_eq!(on, ["symbol_kind", "live"]);
    }

    #[test]
    fn merge_appends_only_what_is_read() {
        let user = parse_program("<test>", "x(r) :- unhandled_route(r).").expect("parses");
        let m = merge(user, &[]).expect("merges");
        let heads: Vec<String> = m.program.rules.iter().map(|r| r.head.signature()).collect();
        // Prelude rules join in prelude order.
        assert_eq!(
            heads,
            ["x/1", "handles/2", "handled/1", "unhandled_route/1"]
        );
        assert_eq!(m.sinks, ["x/1"]);
        assert_eq!(m.program.sources, ["<test>", "prelude"]);
        // An output names a prelude predicate in; nothing else joins.
        let none = parse_program("<test>", "y(a) :- node(a, \"ROUTE\").").expect("parses");
        let m = merge(none.clone(), &[]).expect("merges");
        assert_eq!(m.program.rules.len(), 1);
        assert_eq!(m.program.sources, ["<test>"]);
        let m = merge(none, &["calls/2".to_string()]).expect("merges");
        let heads: Vec<String> = m.program.rules.iter().map(|r| r.head.signature()).collect();
        assert_eq!(heads, ["y/1", "calls/2"]);
        // A negated engine relation's cone reaches its categories through the rules.
        let user = parse_program(
            "<test>",
            "z(n) :- node(n, _), !u(n).\nu(n) :- unreached(n).",
        )
        .expect("parses");
        let m = merge(user, &[]).expect("merges");
        assert_eq!(
            relation_reads(&m.program, "u/1").len(),
            CODE_PROFILE.tables.carry_edges.len()
        );
        assert_eq!(
            relation_reads(&m.program, "live/1"),
            relation_reads(&m.program, "u/1")
        );
        assert!(relation_reads(&m.program, "handled/1").is_empty());
    }
}
