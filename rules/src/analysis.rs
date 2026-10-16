//! Name and type resolution against the [`Catalog`], safety / range restriction,
//! arity, negation and aggregate checks through the strata (DA.5a), the
//! unseeded-recursion and singleton warnings, the negation cones behind Q5's absence
//! caveats, the one sideways-information-passing order the planner (DA.10) and the
//! magic rewrite (DA.12) both read (`sips_order`), and the message table (DA.5b).
//!
//! [`analyse`] turns a string-named [`Program`] into an [`Analysis`]: every predicate
//! resolved to a `PredId`, every rule compiled to a `CRule` whose variables are
//! `u16` slots and whose typed constants are checked against the domain's registries,
//! and the IDB predicates stratified. Any error makes the result
//! `RulesError::Invalid(errors)`, each an Error [`Diagnostic`] at the offending span
//! naming the rule's head `name/arity`, in the order recorded (rule order, then the
//! check order within a rule, then the stratification errors); warnings ride an `Ok`
//! (`Analysis::warnings`: the singleton warnings in rule order, then the unseeded ones
//! in stratum order) and are dropped with an `Invalid`.
//!
//! Predicate ids: the IDB predicates come first, in first-definition order (the order
//! their first head appears), so they are exactly the predicates DA.5a's `stratify`
//! ranges over; the EDB predicates and builtins a body references follow, in the order
//! of their first reference. A builtin is one id per declaration (`reaches/2` and
//! `reaches/3` are two). Every predicate list is a `Vec` in that order and every name
//! is found through `value::Lookup`, so nothing here iterates a hash map: an analysis
//! is a function of the program text.
//!
//! The message texts live in one table (`msg` below). They are an interface: DA.16
//! (rules/tests/errors.rs) pins each end to end, and DB's `check` errors, its
//! `validate_entry` and the `glia gaps` rot report print them verbatim, so a wording
//! change moves all of them together.

use glia_core::EdgeCategoryId;

use crate::diag::{Diagnostic, RulesError, Span};
use crate::ir::{
    AggOp, Atom, BindReq, BuiltinKind, Catalog, CmpOp, ColType, HeadArg, Literal, Program, Rule,
    Term,
};
use crate::strata::{BadCycle, Dep, DepEdge, stratify};
use crate::value::Lookup;

/// Analyse `program` against `catalog`: `analyse_with` with no synthetic (`__`)
/// predicate allowed. `Err(RulesError::Invalid(errors))` when any check fails;
/// warnings ride the `Ok`.
pub fn analyse(program: &Program, catalog: &Catalog) -> Result<Analysis, RulesError> {
    analyse_with(program, catalog, false)
}

/// [`analyse`], with `allow_synthetic` accepting heads that start with the reserved
/// `__` prefix and marking them synthetic. Only the magic rewrite's output (DA.12,
/// re-analysed by DA.14b) passes `true`: its adorned copies `__a_<p>__<adornment>` and
/// magic predicates `__m_<p>__<adornment>` take the prefix no user rule may use.
pub(crate) fn analyse_with(
    program: &Program,
    catalog: &Catalog,
    allow_synthetic: bool,
) -> Result<Analysis, RulesError> {
    let mut cx = Cx::new(program, catalog);
    let heads = cx.heads(allow_synthetic);
    cx.n_idb = cx.preds.len();
    let mut rules = Vec::with_capacity(program.rules.len());
    for (index, rule) in program.rules.iter().enumerate() {
        if let Some(r) = cx.rule(index, rule, heads.get(index).copied().flatten()) {
            rules.push(r);
        }
    }
    cx.aggregate_rule_counts(&rules);
    let strata = cx.strata(&rules);
    // A rule without a CRule recorded an error, so `errors` decides validity alone.
    let strata = match strata {
        Some(s) if cx.errors.is_empty() => s,
        _ => return Err(RulesError::Invalid(cx.errors)),
    };
    cx.unseeded(&rules, &strata);
    let reads = cx.direct_reads(&rules);
    for r in &mut rules {
        r.negated_reads = r
            .body
            .iter()
            .filter_map(|lit| match lit {
                CLit::Neg(p, terms, _) => Some(cx.neg_read(*p, terms, &reads)),
                _ => None,
            })
            .collect();
    }
    Ok(Analysis { warnings: cx.warnings, preds: cx.preds, rules, strata })
}

/// A checked program: its predicates, compiled rules and strata, and the warnings
/// analysis raised.
#[derive(Clone, Debug)]
pub struct Analysis {
    /// Singleton and unseeded-recursion warnings, in the order the module doc gives.
    pub warnings: Vec<Diagnostic>,
    /// By `PredId`.
    pub(crate) preds: Vec<PredInfo>,
    /// One per program rule, in program order (`rules[i].index == i`).
    pub(crate) rules: Vec<CRule>,
    /// Lowest first; each IDB predicate's `stratum` indexes it.
    pub(crate) strata: Vec<Stratum>,
}

impl Analysis {
    /// The user's IDB predicates as `name/arity`, in source (first-definition) order;
    /// synthetic predicates are left out.
    pub fn predicates(&self) -> Vec<String> {
        self.preds
            .iter()
            .filter(|p| p.kind == PredKind::Idb && !p.synthetic)
            .map(PredInfo::signature)
            .collect()
    }

    /// The number of strata: one per strongly connected component of the IDB
    /// dependency graph (DA.5a).
    pub fn strata_count(&self) -> usize {
        self.strata.len()
    }
}

/// A predicate's index into `Analysis::preds`.
pub(crate) type PredId = u32;

/// The head id of a rule whose head did not resolve. Such a rule recorded an error,
/// so its program is Invalid and the placeholder never leaves analysis.
const NO_PRED: PredId = PredId::MAX;

/// The most arguments a predicate may take: relation and plan columns are `u8`
/// (DA.7, DA.10), so column 255 is the last one an index or a plan step can name.
const MAX_ARITY: usize = 256;

/// One predicate the program defines or reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PredInfo {
    pub(crate) name: String,
    pub(crate) arity: usize,
    pub(crate) kind: PredKind,
    /// An IDB predicate's stratum (an index into `Analysis::strata`); 0 for an EDB
    /// predicate or builtin, whose rows are complete before stratum 0 runs.
    pub(crate) stratum: usize,
    /// In a recursive component (two or more predicates, or one reading itself).
    pub(crate) recursive: bool,
    /// The name starts `__`: magic or hoist output, accepted only by
    /// `analyse_with(.., true)`.
    pub(crate) synthetic: bool,
}

impl PredInfo {
    /// `name/arity`.
    pub(crate) fn signature(&self) -> String {
        format!("{}/{}", self.name, self.arity)
    }

    /// Column `i`'s declared type: the catalog's for an EDB predicate or builtin,
    /// `Any` for an IDB predicate (and past a declaration's arity).
    pub(crate) fn col_type(&self, catalog: &Catalog, i: usize) -> ColType {
        let cols = match self.kind {
            PredKind::Idb => None,
            PredKind::Edb(EdbPred::Attr(d)) => catalog.edb.get(d).map(|p| &p.cols),
            PredKind::Edb(_) => catalog.edb(&self.name).map(|p| &p.cols),
            PredKind::Builtin(b) => catalog.builtins.get(b).map(|d| &d.cols),
        };
        cols.and_then(|c| c.get(i)).copied().unwrap_or(ColType::Any)
    }

    /// What a builtin needs of argument `i` before the call; `None` for a relation.
    pub(crate) fn bind_req(&self, catalog: &Catalog, i: usize) -> Option<BindReq> {
        match self.kind {
            PredKind::Builtin(b) => catalog.builtins.get(b).and_then(|d| d.bind.get(i)).copied(),
            PredKind::Idb | PredKind::Edb(_) => None,
        }
    }
}

/// Where a predicate's rows come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PredKind {
    /// Defined by the program's rules.
    Idb,
    /// Read from the fact source.
    Edb(EdbPred),
    /// A builtin: its index into `Catalog::builtins`.
    Builtin(usize),
}

/// An EDB predicate: the core eight, or a domain attribute. DA.6's loader is keyed by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EdbPred {
    Node,
    Edge,
    EdgeConf,
    Cell,
    EdgeCell,
    Kind,
    Category,
    CarryCategory,
    /// A domain attribute: its declaration's index into `Catalog::edb` (past the core
    /// eight). `Catalog::with_attrs` drops an attribute whose name is taken, so the
    /// loader finds the column among `FactSource::attributes()` by this declaration's
    /// name, which is unique.
    Attr(usize),
}

impl EdbPred {
    /// The EDB predicate the catalog declares at `index` under `name`.
    fn of(index: usize, name: &str) -> EdbPred {
        match name {
            "node" => EdbPred::Node,
            "edge" => EdbPred::Edge,
            "edge_conf" => EdbPred::EdgeConf,
            "cell" => EdbPred::Cell,
            "edge_cell" => EdbPred::EdgeCell,
            "kind" => EdbPred::Kind,
            "category" => EdbPred::Category,
            "carry_category" => EdbPred::CarryCategory,
            _ => EdbPred::Attr(index),
        }
    }

    /// One row per edge: its category column names the edge category read.
    fn reads_edges(self) -> bool {
        matches!(self, EdbPred::Edge | EdbPred::EdgeConf | EdbPred::EdgeCell)
    }
}

/// One compiled rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CRule {
    /// Its index in `Program::rules`.
    pub(crate) index: usize,
    pub(crate) head: PredId,
    /// One term per head argument. An aggregate's slot holds `CTerm::Wild` (a `_` is
    /// an error in a head, so a Wild here is always an aggregate): the j-th Wild is
    /// `aggs[j]`.
    pub(crate) head_terms: Vec<CTerm>,
    /// The head's aggregates in head order: the operator and its variables.
    pub(crate) aggs: Vec<(AggOp, Vec<u16>)>,
    /// The body literals in source order.
    pub(crate) body: Vec<CLit>,
    pub(crate) nvars: u16,
    /// Variable `v`'s name is `var_names[v]`, in first-occurrence order (head first).
    pub(crate) var_names: Vec<String>,
    pub(crate) span: Span,
    /// One per negated literal, in body order: what its absence depends on (Q5).
    pub(crate) negated_reads: Vec<NegRead>,
}

/// A compiled term.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CTerm {
    Var(u16),
    Wild,
    Const(ConstVal),
}

/// A constant. A kind / category / cell-type name stays a `Str` (the loader's rows
/// carry names); a category-set argument is parsed once, here.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ConstVal {
    Str(String),
    Int(i64),
    CatSet(CatSet),
}

/// The edge categories a closure builtin walks.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum CatSet {
    /// `"*"`: every category.
    All,
    /// The domain's carry edges: a closure builtin with no category argument.
    Carry,
    /// `"A|B|.."`: these, sorted by id, deduplicated.
    Of(Vec<EdgeCategoryId>),
}

impl CatSet {
    /// The category names the set reads, sorted (an unregistered id renders `#<id>`),
    /// and whether it reads every category.
    fn names(&self, catalog: &Catalog) -> (Vec<String>, bool) {
        let name = |c: &EdgeCategoryId| {
            catalog.registries.category_name(*c).map_or_else(|| format!("#{}", c.0), str::to_string)
        };
        let mut names: Vec<String> = match self {
            CatSet::All => return (Vec::new(), true),
            CatSet::Carry => catalog.carry.iter().map(name).collect(),
            CatSet::Of(ids) => ids.iter().map(name).collect(),
        };
        names.sort();
        names.dedup();
        (names, false)
    }
}

/// A compiled body literal: a positive or negated atom (relation or builtin), or a
/// comparison, each with its span.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CLit {
    Pos(PredId, Vec<CTerm>, Span),
    Neg(PredId, Vec<CTerm>, Span),
    Cmp(CmpOp, CTerm, CTerm, Span),
}

/// One stratum: a strongly connected component of the IDB dependency graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stratum {
    /// Ascending.
    pub(crate) preds: Vec<PredId>,
    /// Its rules run as a fixpoint.
    pub(crate) recursive: bool,
    /// Its predicate is defined by an aggregate rule (then it is the only one, and not
    /// recursive).
    pub(crate) aggregate: bool,
}

/// What a negated atom's absence depends on: the edge categories its predicate reads,
/// transitively through its defining rules. A static cone, so an over-approximation
/// by design; DA.14b turns it into a row's `AbsenceCaveat`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NegRead {
    /// `name/arity`.
    pub(crate) negated: String,
    /// The constant category names read, sorted, deduplicated.
    pub(crate) categories: Vec<String>,
    /// Some read names no constant category (`"*"`, a variable or `_`).
    pub(crate) any_category: bool,
}

/// The category set a closure builtin (`reaches` / `scc`) call walks: its category-set
/// constant, or the carry set when the declaration takes no category argument; a
/// non-constant set (an Invalid program) reads as every category. `None` for any other
/// builtin.
pub(crate) fn closure_set(catalog: &Catalog, builtin: usize, terms: &[CTerm]) -> Option<CatSet> {
    let decl = catalog.builtins.get(builtin)?;
    if !matches!(decl.kind, BuiltinKind::Reaches | BuiltinKind::Scc) {
        return None;
    }
    Some(match decl.cols.iter().position(|c| *c == ColType::CategorySet) {
        None => CatSet::Carry,
        Some(c) => match terms.get(c) {
            Some(CTerm::Const(ConstVal::CatSet(set))) => set.clone(),
            _ => CatSet::All,
        },
    })
}

// ============================================================================
// SIPS: the bound-first literal order
// ============================================================================

/// The order a rule's body literals run in, as indexes into `rule.body`: the one
/// sideways-information-passing order the planner (DA.10), the magic rewrite's
/// adornments (DA.12) and this module's builtin-binding check all read.
///
/// `bound` marks the variables bound before the body runs (missing entries are free);
/// `first`, when given, runs first (the planner's delta literal). Then, greedily:
/// any literal binding no new variable (a pure filter: a membership atom, a comparison
/// or negation over bound variables, a fully bound builtin) in source order; else,
/// among the eligible literals, one JOINED to what is bound (a column holding a
/// variable already bound, or an assignment, which yields one binding per input)
/// before any that is not, so a literal held only by constants (`seg(b, "engine")`
/// after `seg(a, "parsers")` bound `a`) never runs as a cross product while a join
/// on `a` waits; then the most bound columns (constants and bound variables; a
/// builtin's `Const` columns, such as a closure's category set, never count), ties
/// to the smaller known `size` (unknown = `usize::MAX`; an assignment is 1), then
/// source order. A builtin
/// whose `Must` / `Const` columns are not all bound is not eligible, nor is a negation
/// or a comparison with a free variable, except an `=` whose other side is bound (an
/// assignment). When nothing is eligible the first remaining literal runs, so the
/// result is always a permutation; analysis reports what that leaves unbound.
pub(crate) fn sips_order(
    rule: &CRule,
    preds: &[PredInfo],
    catalog: &Catalog,
    bound: &[bool],
    first: Option<usize>,
    size: &dyn Fn(PredId) -> Option<usize>,
) -> Vec<usize> {
    let n = rule.body.len();
    let mut bound: Vec<bool> =
        (0..usize::from(rule.nvars)).map(|v| bound.get(v).copied().unwrap_or(false)).collect();
    let mut placed = vec![false; n];
    let mut order = Vec::with_capacity(n);
    let mut next = first.filter(|&f| f < n);
    while let Some(i) = next.take().or_else(|| pick(rule, preds, catalog, &bound, &placed, size)) {
        placed[i] = true;
        order.push(i);
        bind_literal(&rule.body[i], &mut bound);
    }
    order
}

/// How a literal ranks under the current bindings.
struct Shape {
    /// Binds no new variable and can run.
    filter: bool,
    /// Can run now, binding something.
    eligible: bool,
    /// Reads a variable already bound (or is an assignment): it extends the
    /// bindings so far instead of multiplying them.
    joined: bool,
    /// Constants and bound variables among its columns.
    bound: usize,
    size: usize,
}

/// The next literal [`sips_order`] runs; `None` once every literal is placed.
fn pick(
    rule: &CRule,
    preds: &[PredInfo],
    catalog: &Catalog,
    bound: &[bool],
    placed: &[bool],
    size: &dyn Fn(PredId) -> Option<usize>,
) -> Option<usize> {
    let open: Vec<usize> = (0..rule.body.len()).filter(|&i| !placed[i]).collect();
    let shapes: Vec<Shape> =
        open.iter().map(|&i| shape(&rule.body[i], preds, catalog, bound, size)).collect();
    if let Some(k) = shapes.iter().position(|s| s.filter) {
        return Some(open[k]);
    }
    let mut best: Option<usize> = None;
    for (k, s) in shapes.iter().enumerate() {
        if !s.eligible {
            continue;
        }
        let better = match best {
            None => true,
            Some(b) => {
                let o = &shapes[b];
                (s.joined, s.bound, std::cmp::Reverse(s.size))
                    > (o.joined, o.bound, std::cmp::Reverse(o.size))
            }
        };
        if better {
            best = Some(k);
        }
    }
    best.or(if open.is_empty() { None } else { Some(0) }).map(|k| open[k])
}

/// A literal's [`Shape`] under `bound`.
fn shape(
    lit: &CLit,
    preds: &[PredInfo],
    catalog: &Catalog,
    bound: &[bool],
    size: &dyn Fn(PredId) -> Option<usize>,
) -> Shape {
    match lit {
        CLit::Pos(p, terms, _) => {
            let info = preds.get(*p as usize);
            let req = |c: usize| info.and_then(|i| i.bind_req(catalog, c));
            let runnable = terms.iter().enumerate().all(|(c, t)| match req(c) {
                Some(BindReq::Must | BindReq::Const) => is_bound(t, bound),
                Some(BindReq::May) | None => true,
            });
            // A builtin's `Const` column (a closure's category set or depth) is a literal
            // in every call, so it selects nothing: counting it ranked `reaches(a, b,
            // "CALLS")` with both ends free above a literal that binds one end.
            let selective =
                |(c, t): &(usize, &CTerm)| req(*c) != Some(BindReq::Const) && is_bound(t, bound);
            Shape {
                filter: runnable && !has_free_var(terms, bound),
                eligible: runnable,
                joined: terms.iter().any(|t| matches!(t, CTerm::Var(_)) && is_bound(t, bound)),
                bound: terms.iter().enumerate().filter(selective).count(),
                size: size(*p).unwrap_or(usize::MAX),
            }
        }
        CLit::Neg(_, terms, _) => Shape {
            filter: !has_free_var(terms, bound),
            eligible: false,
            joined: false,
            bound: 0,
            size: usize::MAX,
        },
        CLit::Cmp(op, l, r, _) => {
            let (lb, rb) = (is_bound(l, bound), is_bound(r, bound));
            let assigns = *op == CmpOp::Eq
                && ((matches!(l, CTerm::Var(_)) && !lb && rb)
                    || (matches!(r, CTerm::Var(_)) && !rb && lb));
            // An assignment yields exactly one binding per input binding: size 1, and
            // never a cross product.
            Shape { filter: lb && rb, eligible: assigns, joined: assigns, bound: 1, size: 1 }
        }
    }
}

/// A constant, or a variable already bound. `_` is never bound.
fn is_bound(t: &CTerm, bound: &[bool]) -> bool {
    match t {
        CTerm::Const(_) => true,
        CTerm::Var(v) => bound.get(usize::from(*v)).copied().unwrap_or(false),
        CTerm::Wild => false,
    }
}

fn has_free_var(terms: &[CTerm], bound: &[bool]) -> bool {
    terms.iter().any(|t| matches!(t, CTerm::Var(_)) && !is_bound(t, bound))
}

/// Mark what running `lit` binds: every variable of a positive atom; the free side of
/// an assignment.
fn bind_literal(lit: &CLit, bound: &mut [bool]) {
    let targets: Vec<&CTerm> = match lit {
        CLit::Pos(_, terms, _) => terms.iter().collect(),
        CLit::Cmp(CmpOp::Eq, l, r, _) => match (is_bound(l, bound), is_bound(r, bound)) {
            (true, false) => vec![r],
            (false, true) => vec![l],
            _ => Vec::new(),
        },
        CLit::Neg(..) | CLit::Cmp(..) => Vec::new(),
    };
    for t in targets {
        if let CTerm::Var(v) = t
            && let Some(b) = bound.get_mut(usize::from(*v))
        {
            *b = true;
        }
    }
}

// ============================================================================
// The message table
// ============================================================================

/// Every analysis diagnostic's text. DA.16 pins each end to end; DB's `check`, its
/// `validate_entry` and the `glia gaps` rot report print them verbatim. The
/// ` (in the rule for `p/2`)` suffix is `Diagnostic`'s Display, not part of a message.
mod msg {
    /// `<text>; did you mean `<s>`?`, or `<text>` alone with no suggestion.
    pub(super) fn suggest(text: String, suggestion: Option<&str>) -> String {
        match suggestion {
            Some(s) => format!("{text}; did you mean `{s}`?"),
            None => text,
        }
    }

    pub(super) fn unknown_predicate(sig: &str, suggestion: Option<&str>) -> String {
        suggest(format!("unknown predicate `{sig}`"), suggestion)
    }

    pub(super) fn arity(name: &str, used: usize, defined: &str) -> String {
        let s = if used == 1 { "" } else { "s" };
        format!(
            "predicate `{name}` is used with {used} argument{s} here but defined with {defined}"
        )
    }

    pub(super) fn unknown_kind(name: &str, suggestion: Option<&str>) -> String {
        suggest(format!("unknown node kind `{name}`"), suggestion)
    }

    pub(super) fn unknown_category(name: &str, suggestion: Option<&str>) -> String {
        suggest(format!("unknown edge category `{name}`"), suggestion)
    }

    pub(super) fn unknown_cell_type(name: &str, suggestion: Option<&str>) -> String {
        suggest(format!("unknown cell type `{name}`"), suggestion)
    }

    pub(super) fn unknown_in_set(name: &str, set: &str, suggestion: Option<&str>) -> String {
        suggest(format!("unknown edge category `{name}` in the category set `{set}`"), suggestion)
    }

    pub(super) fn builtin_head(sig: &str) -> String {
        format!("`{sig}` is a built-in predicate and cannot be defined by a rule")
    }

    pub(super) fn reserved() -> String {
        "predicate names starting `__` are reserved".to_string()
    }

    /// `what` is `an integer literal`, `a quoted category set`, `a quoted node kind`, ...
    pub(super) fn const_arg(argno: usize, sig: &str, what: &str) -> String {
        format!("argument {argno} of `{sig}` must be {what}")
    }

    pub(super) fn unbound_head(var: &str, sig: &str) -> String {
        format!("variable `{var}` in the head of `{sig}` is not bound by the body")
    }

    pub(super) fn unbound_negated(var: &str, sig: &str) -> String {
        format!("variable `{var}` in the negated atom `{sig}` is not bound by a positive atom")
    }

    pub(super) fn unbound_comparison(var: &str) -> String {
        format!("variable `{var}` in a comparison is not bound by a positive atom")
    }

    pub(super) fn wild_head() -> String {
        "`_` may not appear in a head".to_string()
    }

    pub(super) fn wild_comparison() -> String {
        "`_` may not appear in a comparison".to_string()
    }

    pub(super) fn too_many_args(name: &str, arity: usize) -> String {
        format!("a predicate may have at most {} arguments; `{name}` has {arity}", super::MAX_ARITY)
    }

    /// `count` with no variable, or `min` / `max` / `sum` with other than one.
    pub(super) fn aggregate_vars(op: super::AggOp, found: usize) -> String {
        match op {
            super::AggOp::Count => "`count` takes at least one variable".to_string(),
            _ => format!("`{op}` takes one variable, found {found}"),
        }
    }

    pub(super) fn needs_bound(sig: &str, argno: usize) -> String {
        format!("`{sig}` needs argument {argno} bound before the call")
    }

    pub(super) fn aggregate_rules(sig: &str, found: usize) -> String {
        format!("`{sig}` has an aggregate head, so it must have exactly one rule (found {found})")
    }

    pub(super) fn unbound_aggregated(var: &str, sig: &str) -> String {
        format!("aggregated variable `{var}` of `{sig}` is not bound by the body")
    }

    /// `preds` are the component's `name/arity`s, sorted by name.
    pub(super) fn negation_cycle(preds: &[String]) -> String {
        format!("negation inside recursion: {} through this negated atom", depend(preds))
    }

    pub(super) fn aggregate_cycle(preds: &[String]) -> String {
        format!("aggregation inside recursion: {} through this aggregate", depend(preds))
    }

    /// `` `p/1` depends on itself `` or `` `p/0`, `q/0` depend on each other ``.
    fn depend(preds: &[String]) -> String {
        let quoted: Vec<String> = preds.iter().map(|p| format!("`{p}`")).collect();
        match quoted.as_slice() {
            [one] => format!("{one} depends on itself"),
            many => format!("{} depend on each other", many.join(", ")),
        }
    }

    pub(super) fn too_many_vars() -> String {
        format!("a rule may use at most {} variables", u16::MAX)
    }

    pub(super) fn singleton(var: &str) -> String {
        format!("variable `{var}` appears once; write `_`")
    }

    /// `sig` the recursive predicate, `from` the predicate its unseeded base rule reads
    /// first, `head` the seeded example head (`tc(x, y)`).
    pub(super) fn unseeded(sig: &str, from: &str, head: &str) -> String {
        format!(
            "recursive predicate `{sig}` is unseeded: its recursion starts from every row of \
             `{from}`; seed it (`{head} :- seed(x), ...`) or use reaches(x, y, \"CATS\"); magic \
             sets seed it when every caller binds an argument"
        )
    }
}

/// The seeded example head for `name/arity`: `tc/2` -> `tc(x, y)`, the variable names
/// the budget error's hint uses (diag.rs), so the warning and the error read alike.
fn example_head(name: &str, arity: usize) -> String {
    const VARS: [&str; 4] = ["x", "y", "z", "w"];
    if arity == 0 {
        return name.to_string();
    }
    let vars: Vec<String> = (0..arity)
        .map(|i| VARS.get(i).map_or_else(|| format!("x{}", i + 1), |v| (*v).to_string()))
        .collect();
    format!("{name}({})", vars.join(", "))
}

/// What a constant in a column of type `ty` must be.
fn const_what(ty: ColType) -> &'static str {
    match ty {
        ColType::Int => "an integer literal",
        ColType::CategorySet => "a quoted category set",
        ColType::Kind => "a quoted node kind",
        ColType::Category => "a quoted edge category",
        ColType::CellType => "a quoted cell type",
        ColType::Str => "a quoted string",
        ColType::Node | ColType::Any => "a literal",
    }
}

/// Levenshtein distance over chars, ASCII case folded (so `route` suggests `ROUTE`).
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().map(|c| c.to_ascii_lowercase()).collect();
    let b: Vec<char> = b.chars().map(|c| c.to_ascii_lowercase()).collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = Vec::with_capacity(b.len() + 1);
        cur.push(i + 1);
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            cur.push(sub.min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// The known name nearest `name` within distance 2 (ties to the smaller name), never
/// `name` itself.
fn suggest<'n>(name: &str, known: impl IntoIterator<Item = &'n str>) -> Option<&'n str> {
    let mut best: Option<(usize, &'n str)> = None;
    for k in known {
        if k == name || k.chars().count().abs_diff(name.chars().count()) > 2 {
            continue;
        }
        let d = distance(name, k);
        if d <= 2 && best.is_none_or(|b| (d, k) < b) {
            best = Some((d, k));
        }
    }
    best.map(|(_, k)| k)
}

// ============================================================================
// The checks
// ============================================================================

/// One rule's variables: names in first-occurrence order, each with its occurrence
/// count and first span (for the singleton warning).
#[derive(Default)]
struct Vars {
    names: Vec<String>,
    count: Vec<u32>,
    first: Vec<Span>,
    overflow: bool,
}

impl Vars {
    /// The slot of `name`, counting this occurrence.
    fn id(&mut self, name: &str, span: Span) -> u16 {
        if let Some(v) = self.find(name) {
            self.count[usize::from(v)] += 1;
            return v;
        }
        match u16::try_from(self.names.len()) {
            Ok(v) if v < u16::MAX => {
                self.names.push(name.to_string());
                self.count.push(1);
                self.first.push(span);
                v
            }
            _ => {
                self.overflow = true;
                0
            }
        }
    }

    fn find(&self, name: &str) -> Option<u16> {
        self.names.iter().position(|n| n == name).and_then(|v| u16::try_from(v).ok())
    }

    fn len(&self) -> u16 {
        u16::try_from(self.names.len()).unwrap_or(u16::MAX)
    }
}

/// A compiled term with no column type (a head, a comparison, an unresolved atom).
fn plain_term(t: &Term, vars: &mut Vars) -> CTerm {
    match t {
        Term::Var { name, span } => CTerm::Var(vars.id(name, *span)),
        Term::Wild(_) => CTerm::Wild,
        Term::Str { value, .. } => CTerm::Const(ConstVal::Str(value.clone())),
        Term::Int { value, .. } => CTerm::Const(ConstVal::Int(*value)),
    }
}

/// Which registry a name column checks against.
#[derive(Clone, Copy)]
enum Registry {
    Kind,
    Category,
    CellType,
}

/// One IDB predicate's direct reads: edge categories and the IDB predicates its rules
/// read (both polarities).
#[derive(Clone, Default)]
struct Reads {
    categories: Vec<String>,
    any: bool,
    idb: Vec<PredId>,
}

/// The analysis state.
struct Cx<'a> {
    program: &'a Program,
    catalog: &'a Catalog,
    errors: Vec<Diagnostic>,
    warnings: Vec<Diagnostic>,
    preds: Vec<PredInfo>,
    /// IDB and referenced EDB predicates by name (a builtin's name spans arities, so
    /// builtins are found through `catalog` and `builtin_ids`).
    by_name: Lookup<String, PredId>,
    /// Per catalog builtin: its id, once a body references it.
    builtin_ids: Vec<Option<PredId>>,
    /// The IDB predicates are ids `0..n_idb`.
    n_idb: usize,
}

impl<'a> Cx<'a> {
    fn new(program: &'a Program, catalog: &'a Catalog) -> Self {
        Cx {
            program,
            catalog,
            errors: Vec::new(),
            warnings: Vec::new(),
            preds: Vec::new(),
            by_name: Lookup::new(),
            builtin_ids: vec![None; catalog.builtins.len()],
            n_idb: 0,
        }
    }

    fn error(&mut self, span: Span, rule_sig: &str, message: String) {
        let source = self.program.source_name(span.source);
        self.errors.push(Diagnostic::error(span, source, Some(rule_sig), message));
    }

    fn warning(&mut self, span: Span, rule_sig: &str, message: String) {
        let source = self.program.source_name(span.source);
        self.warnings.push(Diagnostic::warning(span, source, Some(rule_sig), message));
    }

    fn new_pred(&mut self, name: &str, arity: usize, kind: PredKind, synthetic: bool) -> PredId {
        // Fewer than u32::MAX predicates: each is a clause or a catalog declaration.
        let id = self.preds.len() as PredId;
        self.preds.push(PredInfo {
            name: name.to_string(),
            arity,
            kind,
            stratum: 0,
            recursive: false,
            synthetic,
        });
        id
    }

    fn info(&self, p: PredId) -> Option<&PredInfo> {
        self.preds.get(p as usize)
    }

    fn is_idb(&self, p: PredId) -> bool {
        (p as usize) < self.n_idb
    }

    /// Check (c) and allocate the IDB predicates, in first-definition order: per rule,
    /// its head's id, `None` when the head cannot define a predicate (an EDB / builtin
    /// name, or the arity of an earlier definition differs). A first definition wider
    /// than [`MAX_ARITY`] is an error but keeps its id.
    fn heads(&mut self, allow_synthetic: bool) -> Vec<Option<PredId>> {
        let catalog = self.catalog;
        let mut out = Vec::with_capacity(self.program.rules.len());
        for rule in &self.program.rules {
            let (name, arity, sig) = (&rule.head.pred, rule.head.arity(), rule.head.signature());
            if catalog.edb(name).is_some() || catalog.builtins.iter().any(|b| &b.name == name) {
                self.error(rule.head.span, &sig, msg::builtin_head(&sig));
                out.push(None);
                continue;
            }
            let synthetic = name.starts_with("__");
            if let Some(&id) = self.by_name.get(name.as_str()) {
                let defined = self.preds[id as usize].arity;
                if defined != arity {
                    self.error(rule.head.span, &sig, msg::arity(name, arity, &defined.to_string()));
                    out.push(None);
                    continue;
                }
                out.push(Some(id));
                continue;
            }
            if synthetic && !allow_synthetic {
                self.error(rule.head.span, &sig, msg::reserved());
            }
            if arity > MAX_ARITY {
                self.error(rule.head.span, &sig, msg::too_many_args(name, arity));
            }
            // Held even when reserved or too wide, so the bodies that read it raise no
            // second error.
            let id = self.new_pred(name, arity, PredKind::Idb, synthetic);
            self.by_name.insert(name.clone(), id);
            out.push(Some(id));
        }
        out
    }

    /// The id of catalog builtin `b`, allocating it on first reference.
    fn builtin_pred(&mut self, b: usize) -> PredId {
        if let Some(Some(id)) = self.builtin_ids.get(b) {
            return *id;
        }
        let catalog = self.catalog;
        let decl = &catalog.builtins[b];
        let id = self.new_pred(&decl.name, decl.cols.len(), PredKind::Builtin(b), false);
        self.builtin_ids[b] = Some(id);
        id
    }

    /// Checks (a) and (b) for one body atom: its predicate, or `None` after recording
    /// why it has none.
    fn resolve(&mut self, a: &Atom, rule_sig: &str) -> Option<PredId> {
        let catalog = self.catalog;
        let arity = a.args.len();
        if let Some(&id) = self.by_name.get(a.pred.as_str()) {
            let defined = self.preds[id as usize].arity;
            if defined == arity {
                return Some(id);
            }
            self.error(a.span, rule_sig, msg::arity(&a.pred, arity, &defined.to_string()));
            return None;
        }
        if let Some(i) = catalog.edb.iter().position(|p| p.name == a.pred) {
            let defined = catalog.edb[i].cols.len();
            let id = self.new_pred(&a.pred, defined, PredKind::Edb(EdbPred::of(i, &a.pred)), false);
            self.by_name.insert(a.pred.clone(), id);
            if defined == arity {
                return Some(id);
            }
            self.error(a.span, rule_sig, msg::arity(&a.pred, arity, &defined.to_string()));
            return None;
        }
        let mut arities = Vec::new();
        for (b, decl) in catalog.builtins.iter().enumerate() {
            if decl.name != a.pred {
                continue;
            }
            if decl.cols.len() == arity {
                // A domain declaration wider than a plan can address: refused here, at
                // the call, as an IDB head is in `heads`.
                if arity > MAX_ARITY {
                    self.error(a.span, rule_sig, msg::too_many_args(&a.pred, arity));
                    return None;
                }
                return Some(self.builtin_pred(b));
            }
            arities.push(decl.cols.len());
        }
        if !arities.is_empty() {
            arities.sort_unstable();
            arities.dedup();
            let defined = match arities.as_slice() {
                [init @ .., last] if !init.is_empty() => {
                    let init: Vec<String> = init.iter().map(ToString::to_string).collect();
                    format!("{} or {last}", init.join(", "))
                }
                _ => arities.iter().map(ToString::to_string).collect::<Vec<_>>().join(""),
            };
            self.error(a.span, rule_sig, msg::arity(&a.pred, arity, &defined));
            return None;
        }
        let known = catalog
            .edb
            .iter()
            .map(|p| p.name.as_str())
            .chain(catalog.builtins.iter().map(|b| b.name.as_str()))
            .chain(self.preds[..self.n_idb].iter().map(|p| p.name.as_str()));
        let message = msg::unknown_predicate(&a.signature(), suggest(&a.pred, known));
        self.error(a.span, rule_sig, message);
        None
    }

    /// Check (d) for one argument of a resolved atom: the term, typed by its column.
    fn typed_term(
        &mut self,
        p: PredId,
        col: usize,
        t: &Term,
        vars: &mut Vars,
        rule_sig: &str,
        atom_sig: &str,
    ) -> CTerm {
        let (ty, req) = match self.info(p) {
            Some(info) => (info.col_type(self.catalog, col), info.bind_req(self.catalog, col)),
            None => (ColType::Any, None),
        };
        let argno = col + 1;
        match t {
            Term::Var { span, .. } | Term::Wild(span) => {
                if req == Some(BindReq::Const) {
                    self.error(*span, rule_sig, msg::const_arg(argno, atom_sig, const_what(ty)));
                }
                plain_term(t, vars)
            }
            Term::Str { value, span } => match ty {
                ColType::Kind => self.registry_name(Registry::Kind, value, *span, rule_sig),
                ColType::Category => self.registry_name(Registry::Category, value, *span, rule_sig),
                ColType::CellType => self.registry_name(Registry::CellType, value, *span, rule_sig),
                ColType::CategorySet => match self.category_set(value, *span, rule_sig) {
                    Some(set) => CTerm::Const(ConstVal::CatSet(set)),
                    None => plain_term(t, vars),
                },
                ColType::Int => {
                    self.error(*span, rule_sig, msg::const_arg(argno, atom_sig, const_what(ty)));
                    plain_term(t, vars)
                }
                ColType::Str | ColType::Node | ColType::Any => plain_term(t, vars),
            },
            Term::Int { span, .. } => {
                if !matches!(ty, ColType::Int | ColType::Node | ColType::Any) {
                    self.error(*span, rule_sig, msg::const_arg(argno, atom_sig, const_what(ty)));
                }
                plain_term(t, vars)
            }
        }
    }

    /// A kind / category / cell-type constant: a registry name, or an error naming the
    /// nearest one.
    fn registry_name(&mut self, which: Registry, value: &str, span: Span, rule_sig: &str) -> CTerm {
        let catalog = self.catalog;
        let reg = &catalog.registries;
        let names: Vec<&'static str> = match which {
            Registry::Kind => reg.node_kinds.iter().map(|(_, n)| *n).collect(),
            Registry::Category => reg.edge_categories.iter().map(|(_, n)| *n).collect(),
            Registry::CellType => reg.cell_types.iter().map(|(_, n)| *n).collect(),
        };
        if !names.contains(&value) {
            let near = suggest(value, names.iter().copied());
            let message = match which {
                Registry::Kind => msg::unknown_kind(value, near),
                Registry::Category => msg::unknown_category(value, near),
                Registry::CellType => msg::unknown_cell_type(value, near),
            };
            self.error(span, rule_sig, message);
        }
        CTerm::Const(ConstVal::Str(value.to_string()))
    }

    /// A category-set constant: `*`, or `A|B|..` of registered names (each trimmed).
    fn category_set(&mut self, value: &str, span: Span, rule_sig: &str) -> Option<CatSet> {
        if value.trim() == "*" {
            return Some(CatSet::All);
        }
        let table = self.catalog.registries.edge_categories;
        let mut ids = Vec::new();
        let mut ok = true;
        for part in value.split('|') {
            let name = part.trim();
            match table.iter().find(|(_, n)| *n == name) {
                Some((id, _)) => ids.push(*id),
                None => {
                    let near = suggest(name, table.iter().map(|(_, n)| *n));
                    self.error(span, rule_sig, msg::unknown_in_set(name, value, near));
                    ok = false;
                }
            }
        }
        ids.sort_by_key(|c| c.0);
        ids.dedup();
        ok.then_some(CatSet::Of(ids))
    }

    /// Compile one rule and run its per-rule checks: `_` in a head, each aggregate's
    /// variable count (`count` one or more, `min` / `max` / `sum` exactly one), (a),
    /// (b), (d), (e) with a negated builtin's `_`, and the singleton warning. `None`
    /// when its head did not resolve (an error is recorded).
    fn rule(&mut self, index: usize, rule: &Rule, head: Option<PredId>) -> Option<CRule> {
        let sig = rule.head.signature();
        let mut vars = Vars::default();
        let mut head_terms = Vec::with_capacity(rule.head.args.len());
        let mut aggs = Vec::new();
        for arg in &rule.head.args {
            match arg {
                HeadArg::Term(t) => {
                    if let Term::Wild(span) = t {
                        self.error(*span, &sig, msg::wild_head());
                    }
                    head_terms.push(plain_term(t, &mut vars));
                }
                HeadArg::Agg(a) => {
                    let found = a.vars.len();
                    let fits = if a.op == AggOp::Count { found > 0 } else { found == 1 };
                    if !fits {
                        self.error(a.span, &sig, msg::aggregate_vars(a.op, found));
                    }
                    let ids = a.vars.iter().map(|v| vars.id(v, a.span)).collect();
                    aggs.push((a.op, ids));
                    head_terms.push(CTerm::Wild);
                }
            }
        }
        let mut body = Vec::with_capacity(rule.body.len());
        // Variables of positive atoms that did not resolve: bound, so one unknown name
        // raises one error, not one per variable.
        let mut loose: Vec<u16> = Vec::new();
        for lit in &rule.body {
            match lit {
                Literal::Pos(a) | Literal::Neg(a) => {
                    let positive = matches!(lit, Literal::Pos(_));
                    let pred = self.resolve(a, &sig);
                    let atom_sig = a.signature();
                    let mut terms = Vec::with_capacity(a.args.len());
                    for (col, t) in a.args.iter().enumerate() {
                        terms.push(match pred {
                            Some(p) => self.typed_term(p, col, t, &mut vars, &sig, &atom_sig),
                            None => plain_term(t, &mut vars),
                        });
                    }
                    // A negated builtin runs as a filter over bound columns: `_` in a
                    // `Must` column leaves the call nothing to test (a positive call's `_`
                    // is `builtin_binding`'s, which never counts `_` bound).
                    if let Some(p) = pred
                        && !positive
                    {
                        self.negated_must_wild(p, &a.args, a.span, &sig);
                    }
                    match pred {
                        Some(p) if positive => body.push(CLit::Pos(p, terms, a.span)),
                        Some(p) => body.push(CLit::Neg(p, terms, a.span)),
                        None if positive => loose.extend(terms.iter().filter_map(|t| match t {
                            CTerm::Var(v) => Some(*v),
                            _ => None,
                        })),
                        None => {}
                    }
                }
                Literal::Cmp(c) => {
                    let l = plain_term(&c.left, &mut vars);
                    let r = plain_term(&c.right, &mut vars);
                    body.push(CLit::Cmp(c.op, l, r, c.span));
                }
            }
        }
        if vars.overflow {
            self.error(rule.head.span, &sig, msg::too_many_vars());
        }
        let crule = CRule {
            index,
            head: head.unwrap_or(NO_PRED),
            head_terms,
            aggs,
            body,
            nvars: vars.len(),
            var_names: vars.names.clone(),
            span: rule.span,
            negated_reads: Vec::new(),
        };
        self.safety(rule, &crule, &vars, &loose, &sig);
        self.builtin_binding(&crule, &sig);
        for v in 0..vars.names.len() {
            if vars.count[v] == 1 && !vars.names[v].starts_with('_') {
                let message = msg::singleton(&vars.names[v]);
                self.warning(vars.first[v], &sig, message);
            }
        }
        head.map(|_| crule)
    }

    /// Check (e) over the set of bound variables: positive relation atoms and a
    /// builtin's `May` columns bind, then assignments (`y = <bound>`) to a fixpoint;
    /// every head, aggregated, negated and compared variable must be bound, and a
    /// comparison holds no `_`.
    fn safety(&mut self, rule: &Rule, cr: &CRule, vars: &Vars, loose: &[u16], sig: &str) {
        let mut bound = vec![false; usize::from(cr.nvars)];
        let set = |bound: &mut Vec<bool>, v: u16| {
            if let Some(b) = bound.get_mut(usize::from(v)) {
                *b = true;
            }
        };
        for &v in loose {
            set(&mut bound, v);
        }
        for lit in &cr.body {
            if let CLit::Pos(p, terms, _) = lit {
                for (col, t) in terms.iter().enumerate() {
                    let req = self.info(*p).and_then(|i| i.bind_req(self.catalog, col));
                    if let CTerm::Var(v) = t
                        && req.is_none_or(|r| r == BindReq::May)
                    {
                        set(&mut bound, *v);
                    }
                }
            }
        }
        loop {
            let mut changed = false;
            for lit in &cr.body {
                if let CLit::Cmp(CmpOp::Eq, l, r, _) = lit {
                    for (a, b) in [(l, r), (r, l)] {
                        if let CTerm::Var(v) = a
                            && !is_bound(a, &bound)
                            && is_bound(b, &bound)
                        {
                            set(&mut bound, *v);
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        let unbound = |name: &str| vars.find(name).is_some_and(|v| !bound[usize::from(v)]);
        let mut errors: Vec<(Span, String)> = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        for arg in &rule.head.args {
            match arg {
                HeadArg::Term(Term::Var { name, span })
                    if unbound(name) && !seen.contains(&name.as_str()) =>
                {
                    seen.push(name);
                    errors.push((*span, msg::unbound_head(name, sig)));
                }
                HeadArg::Agg(a) => {
                    for name in &a.vars {
                        if unbound(name) && !seen.contains(&name.as_str()) {
                            seen.push(name);
                            errors.push((a.span, msg::unbound_aggregated(name, sig)));
                        }
                    }
                }
                HeadArg::Term(_) => {}
            }
        }
        for lit in &rule.body {
            let mut seen: Vec<&str> = Vec::new();
            match lit {
                Literal::Neg(a) => {
                    for t in &a.args {
                        if let Term::Var { name, span } = t
                            && unbound(name)
                            && !seen.contains(&name.as_str())
                        {
                            seen.push(name);
                            errors.push((*span, msg::unbound_negated(name, &a.signature())));
                        }
                    }
                }
                Literal::Cmp(c) => {
                    for t in [&c.left, &c.right] {
                        match t {
                            Term::Var { name, span }
                                if unbound(name) && !seen.contains(&name.as_str()) =>
                            {
                                seen.push(name);
                                errors.push((*span, msg::unbound_comparison(name)));
                            }
                            // `_` stands for no value a comparison could test.
                            Term::Wild(span) => errors.push((*span, msg::wild_comparison())),
                            _ => {}
                        }
                    }
                }
                Literal::Pos(_) => {}
            }
        }
        for (span, message) in errors {
            self.error(span, sig, message);
        }
    }

    /// Check (e)'s builtin half: walking [`sips_order`] from nothing bound, every
    /// builtin's `Must` columns are bound when it is reached.
    fn builtin_binding(&mut self, cr: &CRule, sig: &str) {
        let order = sips_order(cr, &self.preds, self.catalog, &[], None, &|_| None);
        let mut bound = vec![false; usize::from(cr.nvars)];
        let mut errors: Vec<(Span, String)> = Vec::new();
        for i in order {
            let lit = &cr.body[i];
            if let CLit::Pos(p, terms, span) = lit
                && let Some(info) = self.info(*p)
                && let PredKind::Builtin(_) = info.kind
            {
                for (col, t) in terms.iter().enumerate() {
                    if info.bind_req(self.catalog, col) == Some(BindReq::Must)
                        && !is_bound(t, &bound)
                    {
                        errors.push((*span, msg::needs_bound(&info.signature(), col + 1)));
                    }
                }
            }
            bind_literal(lit, &mut bound);
        }
        for (span, message) in errors {
            self.error(span, sig, message);
        }
    }

    /// Check (e) for a negated builtin `!b(args)` (`p`): a `_` in a `Must` column is an
    /// error at the call, worded as `builtin_binding`'s for a positive call.
    fn negated_must_wild(&mut self, p: PredId, args: &[Term], span: Span, sig: &str) {
        let Some(info) = self.info(p) else { return };
        let errors: Vec<String> = args
            .iter()
            .enumerate()
            .filter(|(col, t)| {
                matches!(t, Term::Wild(_))
                    && info.bind_req(self.catalog, *col) == Some(BindReq::Must)
            })
            .map(|(col, _)| msg::needs_bound(&info.signature(), col + 1))
            .collect();
        for message in errors {
            self.error(span, sig, message);
        }
    }

    /// Check (f)'s rule count: a predicate with an aggregate head has that one rule.
    fn aggregate_rule_counts(&mut self, rules: &[CRule]) {
        let program = self.program;
        for p in 0..self.n_idb as PredId {
            let defining: Vec<&CRule> = rules.iter().filter(|r| r.head == p).collect();
            if defining.len() < 2 {
                continue;
            }
            if let Some(agg) = defining.iter().find(|r| !r.aggs.is_empty()) {
                let head = &program.rules[agg.index].head;
                let sig = head.signature();
                self.error(head.span, &sig, msg::aggregate_rules(&sig, defining.len()));
            }
        }
    }

    /// Check (g): one DepEdge per body literal naming an IDB predicate, stratified by
    /// DA.5a; each bad cycle is one error at its site. The strata, with every IDB
    /// predicate's stratum and recursive flag set, or `None` after recording errors.
    fn strata(&mut self, rules: &[CRule]) -> Option<Vec<Stratum>> {
        let mut edges = Vec::new();
        // Per edge: the rule (program index) and the span its error points at.
        let mut sites: Vec<(usize, Span)> = Vec::new();
        for r in rules {
            let agg_span = self.program.rules[r.index].head.args.iter().find_map(|a| match a {
                HeadArg::Agg(a) => Some(a.span),
                HeadArg::Term(_) => None,
            });
            for lit in &r.body {
                let (p, dep, span) = match (lit, agg_span) {
                    (CLit::Pos(p, _, _), Some(agg)) => (*p, Dep::Agg, agg),
                    (CLit::Pos(p, _, span), None) => (*p, Dep::Pos, *span),
                    (CLit::Neg(p, _, span), _) => (*p, Dep::Neg, *span),
                    (CLit::Cmp(..), _) => continue,
                };
                if self.is_idb(p) {
                    edges.push(DepEdge { body: p, head: r.head, dep, site: sites.len() });
                    sites.push((r.index, span));
                }
            }
        }
        let strata = match stratify(self.n_idb, &edges) {
            Ok(s) => s,
            Err(bad) => {
                for b in &bad {
                    self.bad_cycle(b, &sites);
                }
                return None;
            }
        };
        for p in 0..self.n_idb {
            self.preds[p].stratum = strata.stratum_of[p];
            self.preds[p].recursive = strata.recursive[p];
        }
        let has_agg: Vec<bool> = (0..self.n_idb as PredId)
            .map(|p| rules.iter().any(|r| r.head == p && !r.aggs.is_empty()))
            .collect();
        Some(
            strata
                .order
                .into_iter()
                .map(|preds| Stratum {
                    recursive: preds.iter().any(|&p| strata.recursive[p as usize]),
                    aggregate: preds.iter().any(|&p| has_agg[p as usize]),
                    preds,
                })
                .collect(),
        )
    }

    /// One error for a component a negated or aggregated read stays inside.
    fn bad_cycle(&mut self, b: &BadCycle, sites: &[(usize, Span)]) {
        let Some(&(rule, span)) = sites.get(b.site) else { return };
        let mut members: Vec<(&str, usize)> = b
            .preds
            .iter()
            .filter_map(|&p| self.info(p))
            .map(|i| (i.name.as_str(), i.arity))
            .collect();
        members.sort_unstable();
        let names: Vec<String> = members.iter().map(|(n, a)| format!("{n}/{a}")).collect();
        let message = match b.dep {
            Dep::Agg => msg::aggregate_cycle(&names),
            Dep::Neg | Dep::Pos => msg::negation_cycle(&names),
        };
        let sig = self.program.rules[rule].head.signature();
        self.error(span, &sig, message);
    }

    /// Warning (i): a recursive component none of whose base rules (rules reading no
    /// predicate of the component) is seeded.
    fn unseeded(&mut self, rules: &[CRule], strata: &[Stratum]) {
        for (k, st) in strata.iter().enumerate() {
            if !st.recursive {
                continue;
            }
            let in_scc = |p: &PredId| {
                self.is_idb(*p)
                    && self.preds[*p as usize].recursive
                    && self.preds[*p as usize].stratum == k
            };
            let base: Vec<&CRule> = rules
                .iter()
                .filter(|r| st.preds.contains(&r.head))
                .filter(|r| {
                    !r.body
                        .iter()
                        .any(|l| matches!(l, CLit::Pos(p, ..) | CLit::Neg(p, ..) if in_scc(p)))
                })
                .collect();
            if base.is_empty() || base.iter().any(|r| self.seeds(r, k)) {
                continue;
            }
            let r = base[0];
            let from = r.body.iter().find_map(|l| match l {
                CLit::Pos(p, ..) => self.info(*p).map(PredInfo::signature),
                _ => None,
            });
            let (Some(from), Some(head)) = (from, self.info(r.head)) else { continue };
            let (sig, example) = (head.signature(), example_head(&head.name, head.arity));
            let span = self.program.rules[r.index].head.span;
            self.warning(span, &sig, msg::unseeded(&sig, &from, &example));
        }
    }

    /// Does base rule `r` of the stratum-`k` component start from a seed: a fact, no
    /// positive atom at all, a constant argument of a relation atom (not an edge
    /// category, category set or cell type: those choose an edge or cell family, any
    /// row of which can start the recursion), a lower-stratum IDB atom, or a builtin
    /// call with a constant in a `Const` / `Must` column?
    fn seeds(&self, r: &CRule, k: usize) -> bool {
        let mut positive = false;
        for lit in &r.body {
            let CLit::Pos(p, terms, _) = lit else { continue };
            positive = true;
            let Some(info) = self.info(*p) else { continue };
            let constant = |c: usize, t: &CTerm| -> bool {
                if !matches!(t, CTerm::Const(_)) {
                    return false;
                }
                match info.kind {
                    PredKind::Builtin(_) => {
                        matches!(
                            info.bind_req(self.catalog, c),
                            Some(BindReq::Const | BindReq::Must)
                        )
                    }
                    PredKind::Idb | PredKind::Edb(_) => !matches!(
                        info.col_type(self.catalog, c),
                        ColType::Category | ColType::CategorySet | ColType::CellType
                    ),
                }
            };
            if (info.kind == PredKind::Idb && info.stratum < k)
                || terms.iter().enumerate().any(|(c, t)| constant(c, t))
            {
                return true;
            }
        }
        !positive
    }

    /// The edge categories one literal reads by itself: an edge-family atom's category
    /// column, a closure builtin's category set.
    fn literal_reads(&self, p: PredId, terms: &[CTerm], out: &mut Reads) {
        let Some(info) = self.info(p) else { return };
        match info.kind {
            PredKind::Edb(e) if e.reads_edges() => {
                let col =
                    (0..info.arity).find(|&c| info.col_type(self.catalog, c) == ColType::Category);
                match col.and_then(|c| terms.get(c)) {
                    Some(CTerm::Const(ConstVal::Str(name))) => out.categories.push(name.clone()),
                    _ => out.any = true,
                }
            }
            PredKind::Builtin(b) => {
                if let Some(set) = closure_set(self.catalog, b, terms) {
                    let (names, any) = set.names(self.catalog);
                    out.categories.extend(names);
                    out.any |= any;
                }
            }
            PredKind::Edb(_) | PredKind::Idb => {}
        }
    }

    /// Per IDB predicate: what its rules read directly.
    fn direct_reads(&self, rules: &[CRule]) -> Vec<Reads> {
        let mut reads = vec![Reads::default(); self.n_idb];
        for r in rules {
            let Some(slot) = reads.get_mut(r.head as usize) else { continue };
            for lit in &r.body {
                if let CLit::Pos(p, terms, _) | CLit::Neg(p, terms, _) = lit {
                    self.literal_reads(*p, terms, slot);
                    if self.is_idb(*p) && !slot.idb.contains(p) {
                        slot.idb.push(*p);
                    }
                }
            }
        }
        reads
    }

    /// The negation cone of `!p(terms)`: the literal's own reads, plus, for an IDB
    /// predicate, every read of the predicates its rules reach.
    fn neg_read(&self, p: PredId, terms: &[CTerm], reads: &[Reads]) -> NegRead {
        let mut acc = Reads::default();
        self.literal_reads(p, terms, &mut acc);
        if self.is_idb(p) {
            let mut seen = vec![false; self.n_idb];
            let mut stack = vec![p];
            seen[p as usize] = true;
            while let Some(q) = stack.pop() {
                let Some(r) = reads.get(q as usize) else { continue };
                acc.categories.extend(r.categories.iter().cloned());
                acc.any |= r.any;
                for &next in &r.idb {
                    if !seen[next as usize] {
                        seen[next as usize] = true;
                        stack.push(next);
                    }
                }
            }
        }
        acc.categories.sort();
        acc.categories.dedup();
        NegRead {
            negated: self.info(p).map_or_else(String::new, PredInfo::signature),
            categories: acc.categories,
            any_category: acc.any,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::Severity;
    use crate::ir::{BuiltinDecl, Comparison};
    use glia_activation::profile::Registries;
    use glia_core::{CellTypeId, NodeKindId};

    const REG: Registries = Registries {
        node_kinds: &[
            (NodeKindId(1), "FUNCTION"),
            (NodeKindId(2), "MODULE"),
            (NodeKindId(3), "ROUTE"),
        ],
        edge_categories: &[
            (EdgeCategoryId(1), "CALLS"),
            (EdgeCategoryId(2), "IMPORTS"),
            (EdgeCategoryId(3), "USES"),
            (EdgeCategoryId(4), "HANDLED_BY"),
        ],
        cell_types: &[(CellTypeId(1), "CODE")],
    };

    fn catalog() -> Catalog {
        Catalog::core(REG, &[EdgeCategoryId(1), EdgeCategoryId(3)])
    }

    fn in_scope() -> BuiltinDecl {
        BuiltinDecl {
            name: "in_scope".into(),
            cols: vec![ColType::Node, ColType::Str],
            bind: vec![BindReq::May, BindReq::Must],
            tiered: false,
            kind: BuiltinKind::Domain,
        }
    }

    fn v(n: &str) -> Term {
        Term::var(n)
    }

    fn s(x: &str) -> Term {
        Term::str(x)
    }

    fn pos(p: &str, args: Vec<Term>) -> Literal {
        Literal::pos(Atom::new(p, args))
    }

    fn neg(p: &str, args: Vec<Term>) -> Literal {
        Literal::neg(Atom::new(p, args))
    }

    fn rule(head: &str, args: Vec<Term>, body: Vec<Literal>) -> Rule {
        Rule::new(head, args.into_iter().map(HeadArg::Term).collect(), body)
    }

    fn program(rules: Vec<Rule>) -> Program {
        let mut p = Program::new();
        p.add_source("rules");
        p.rules = rules;
        p
    }

    fn at(start: u32) -> Span {
        Span { source: 0, start, end: start + 1, line: 1, col: start + 1 }
    }

    fn ok(p: &Program) -> Analysis {
        ok_in(p, &catalog())
    }

    fn ok_in(p: &Program, c: &Catalog) -> Analysis {
        match analyse(p, c) {
            Ok(a) => a,
            Err(e) => panic!("expected an analysis, got: {e}"),
        }
    }

    fn errors(p: &Program) -> Vec<Diagnostic> {
        errors_in(p, &catalog())
    }

    fn errors_in(p: &Program, c: &Catalog) -> Vec<Diagnostic> {
        match analyse(p, c) {
            Ok(a) => panic!("expected errors, got {:?}", a.predicates()),
            Err(RulesError::Invalid(d)) => d,
            Err(e) => panic!("expected Invalid, got {e:?}"),
        }
    }

    fn messages(d: &[Diagnostic]) -> Vec<&str> {
        d.iter().map(|d| d.message.as_str()).collect()
    }

    fn id_of(a: &Analysis, name: &str) -> PredId {
        let p = a.preds.iter().position(|p| p.name == name);
        p.map(|p| p as PredId).unwrap_or_else(|| panic!("no predicate {name}"))
    }

    fn stratum_of(a: &Analysis, name: &str) -> usize {
        a.preds[id_of(a, name) as usize].stratum
    }

    /// `tc(x, y) :- <base>. tc(x, z) :- tc(x, y), edge(y, z, "CALLS").`
    fn tc(base: Vec<Literal>) -> Vec<Rule> {
        vec![
            rule("tc", vec![v("x"), v("y")], base),
            rule(
                "tc",
                vec![v("x"), v("z")],
                vec![
                    pos("tc", vec![v("x"), v("y")]),
                    pos("edge", vec![v("y"), v("z"), s("CALLS")]),
                ],
            ),
        ]
    }

    #[test]
    fn unknown_predicate_suggests() {
        let p = program(vec![rule(
            "v",
            vec![v("a")],
            vec![pos("edg", vec![v("a"), v("b"), s("CALLS")])],
        )]);
        let d = errors(&p);
        assert_eq!(d.len(), 1, "{d:?}");
        assert_eq!(d[0].message, "unknown predicate `edg/3`; did you mean `edge`?");
        assert_eq!(d[0].predicate.as_deref(), Some("v/1"));
        assert_eq!(d[0].severity, Severity::Error);
        assert_eq!(d[0].source_name, "rules");

        // Nothing within distance 2: no suggestion. Ties go to the smaller name, and a
        // user predicate is a known name too.
        let far = program(vec![rule("v", vec![v("a")], vec![pos("zzzz", vec![v("a")])])]);
        assert_eq!(messages(&errors(&far)), ["unknown predicate `zzzz/1`"]);
        let tie = program(vec![
            rule("hb", vec![v("a")], vec![pos("node", vec![v("a"), Term::wild()])]),
            rule("ha", vec![v("a")], vec![pos("node", vec![v("a"), Term::wild()])]),
            rule("v", vec![v("a")], vec![pos("hc", vec![v("a")])]),
        ]);
        assert_eq!(messages(&errors(&tie)), ["unknown predicate `hc/1`; did you mean `ha`?"]);
        assert_eq!(suggest("Edge", ["edge", "node"]), Some("edge"));
        assert_eq!(distance("ROTUE", "ROUTE"), 2);
        assert_eq!(distance("", "abc"), 3);
    }

    #[test]
    fn arity_mismatch() {
        let p = program(vec![
            rule("p", vec![v("x")], vec![pos("edge", vec![v("x"), Term::wild(), s("CALLS")])]),
            rule("q", vec![v("x")], vec![pos("p", vec![v("x"), v("x")])]),
        ]);
        let d = errors(&p);
        assert_eq!(
            messages(&d),
            ["predicate `p` is used with 2 arguments here but defined with 1"]
        );
        assert_eq!(d[0].predicate.as_deref(), Some("q/1"));

        // Against a declaration, a builtin's arities, and a second head.
        let p = program(vec![
            rule("a", vec![v("x")], vec![pos("edge", vec![v("x"), v("x")])]),
            rule(
                "b",
                vec![v("x")],
                vec![pos("node", vec![v("x"), Term::wild()]), pos("reaches", vec![v("x")])],
            ),
            rule("a", vec![v("x"), v("y")], vec![pos("edge", vec![v("x"), v("y"), s("CALLS")])]),
        ]);
        assert_eq!(
            messages(&errors(&p)),
            [
                "predicate `a` is used with 2 arguments here but defined with 1",
                "predicate `edge` is used with 2 arguments here but defined with 3",
                "predicate `reaches` is used with 1 argument here but defined with 2, 3 or 4",
            ]
        );
    }

    #[test]
    fn unknown_names() {
        let p = program(vec![rule(
            "v",
            vec![v("x")],
            vec![
                pos("node", vec![v("x"), s("ROTUE")]),
                pos("edge", vec![v("x"), Term::wild(), s("CALS")]),
            ],
        )]);
        assert_eq!(
            messages(&errors(&p)),
            [
                "unknown node kind `ROTUE`; did you mean `ROUTE`?",
                "unknown edge category `CALS`; did you mean `CALLS`?",
            ]
        );
        let p = program(vec![rule(
            "v",
            vec![v("a")],
            vec![
                pos("node", vec![v("a"), Term::wild()]),
                pos("reaches", vec![v("a"), v("b"), s("CALLS|NOPE")]),
                pos("cell", vec![v("b"), s("X"), Term::wild()]),
            ],
        )]);
        assert_eq!(
            messages(&errors(&p)),
            [
                "unknown edge category `NOPE` in the category set `CALLS|NOPE`",
                "unknown cell type `X`",
            ]
        );
    }

    #[test]
    fn typed_constants() {
        let set = |text: &str| {
            let p = program(vec![rule(
                "v",
                vec![v("b")],
                vec![
                    pos("node", vec![v("a"), Term::wild()]),
                    pos("reaches", vec![v("a"), v("b"), s(text)]),
                ],
            )]);
            match &ok(&p).rules[0].body[1] {
                CLit::Pos(_, terms, _) => terms[2].clone(),
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(set("*"), CTerm::Const(ConstVal::CatSet(CatSet::All)));
        assert_eq!(
            set("USES | CALLS|USES"),
            CTerm::Const(ConstVal::CatSet(CatSet::Of(vec![EdgeCategoryId(1), EdgeCategoryId(3)])))
        );

        // Const-bound builtin arguments, and constants of the wrong type.
        let p = program(vec![
            rule(
                "v",
                vec![v("b")],
                vec![
                    pos("node", vec![v("a"), Term::wild()]),
                    pos("reaches", vec![v("a"), v("b"), s("CALLS"), v("n")]),
                    pos("reaches", vec![v("a"), v("b"), v("cats")]),
                    pos("node", vec![v("a"), Term::int(3)]),
                    pos("starts_with", vec![v("a"), Term::int(1)]),
                ],
            ),
            rule(
                "w",
                vec![v("a")],
                vec![pos("reaches", vec![v("a"), Term::wild(), s("CALLS"), s("2")])],
            ),
        ]);
        assert_eq!(
            messages(&errors(&p)),
            [
                "argument 4 of `reaches/4` must be an integer literal",
                "argument 3 of `reaches/3` must be a quoted category set",
                "argument 2 of `node/2` must be a quoted node kind",
                "argument 2 of `starts_with/2` must be a quoted string",
                "argument 4 of `reaches/4` must be an integer literal",
            ]
        );
        // Kind and category names stay strings; an int literal is an Int.
        let p = program(vec![rule(
            "v",
            vec![v("a")],
            vec![
                pos("node", vec![v("a"), s("ROUTE")]),
                pos("reaches", vec![v("a"), v("b"), s("CALLS"), Term::int(3)]),
            ],
        )]);
        let a = ok(&p);
        assert_eq!(
            a.rules[0].body[0],
            CLit::Pos(
                id_of(&a, "node"),
                vec![CTerm::Var(0), CTerm::Const(ConstVal::Str("ROUTE".into()))],
                Span::default()
            )
        );
        match &a.rules[0].body[1] {
            CLit::Pos(_, terms, _) => assert_eq!(terms[3], CTerm::Const(ConstVal::Int(3))),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn reserved_heads() {
        let p = program(vec![
            rule(
                "edge",
                vec![v("x"), v("y"), v("c")],
                vec![pos("edge", vec![v("x"), v("y"), v("c")])],
            ),
            rule(
                "reaches",
                vec![v("x"), v("y")],
                vec![pos("edge", vec![v("x"), v("y"), Term::wild()])],
            ),
            rule("ok", vec![v("x")], vec![pos("node", vec![v("x"), Term::wild()])]),
        ]);
        assert_eq!(
            messages(&errors(&p)),
            [
                "`edge/3` is a built-in predicate and cannot be defined by a rule",
                "`reaches/2` is a built-in predicate and cannot be defined by a rule",
            ]
        );
        let attrs =
            catalog().with_attrs(&[crate::ir::AttrDecl { name: "index", ty: ColType::Int }]);
        let p = program(vec![rule(
            "index",
            vec![v("x"), Term::int(0)],
            vec![pos("node", vec![v("x"), Term::wild()])],
        )]);
        assert_eq!(
            messages(&errors_in(&p, &attrs)),
            ["`index/2` is a built-in predicate and cannot be defined by a rule"]
        );
    }

    #[test]
    fn unsafe_head_and_negation() {
        let d = errors(&program(vec![rule(
            "v",
            vec![v("y")],
            vec![pos("node", vec![v("x"), s("ROUTE")])],
        )]));
        assert_eq!(messages(&d), ["variable `y` in the head of `v/1` is not bound by the body"]);

        let defs = || rule("p", vec![v("x")], vec![pos("node", vec![v("x"), Term::wild()])]);
        let d =
            errors(&program(vec![defs(), rule("v", vec![v("x")], vec![neg("p", vec![v("x")])])]));
        assert_eq!(
            messages(&d),
            [
                "variable `x` in the head of `v/1` is not bound by the body",
                "variable `x` in the negated atom `p/1` is not bound by a positive atom",
            ]
        );

        let d = errors(&program(vec![rule(
            "v",
            vec![v("x")],
            vec![
                pos("node", vec![v("x"), Term::wild()]),
                Literal::cmp(CmpOp::Gt, v("y"), Term::int(3)),
            ],
        )]));
        assert_eq!(messages(&d), ["variable `y` in a comparison is not bound by a positive atom"]);

        // An assignment binds: `y = x`, and a chain `z = y` after it.
        let safe = program(vec![rule(
            "v",
            vec![v("x"), v("z")],
            vec![
                Literal::cmp(CmpOp::Eq, v("z"), v("y")),
                pos("node", vec![v("x"), Term::wild()]),
                Literal::cmp(CmpOp::Eq, v("y"), v("x")),
            ],
        )]);
        assert!(ok(&safe).warnings.is_empty());
        let assign = program(vec![rule(
            "v",
            vec![v("x")],
            vec![pos("node", vec![v("x"), Term::wild()]), Literal::cmp(CmpOp::Eq, v("y"), v("x"))],
        )]);
        assert!(analyse(&assign, &catalog()).is_ok());

        // `_` in a head; a fact's variable; a May column binds.
        let d = errors(&program(vec![
            rule("w", vec![Term::wild()], vec![pos("node", vec![v("x"), Term::wild()])]),
            Rule::fact("f", vec![v("q")]),
        ]));
        assert_eq!(
            messages(&d),
            [
                "`_` may not appear in a head",
                "variable `q` in the head of `f/1` is not bound by the body"
            ]
        );
        let c = catalog().with_builtins(vec![in_scope()]);
        assert!(
            analyse(
                &program(vec![rule(
                    "v",
                    vec![v("x")],
                    vec![pos("in_scope", vec![v("x"), s("cli")])]
                )]),
                &c
            )
            .is_ok()
        );
    }

    #[test]
    fn builtin_binding() {
        let c = catalog().with_builtins(vec![in_scope()]);
        let p = program(vec![rule(
            "v",
            vec![v("x")],
            vec![pos("node", vec![v("x"), Term::wild()]), pos("in_scope", vec![v("x"), v("p")])],
        )]);
        let d = errors_in(&p, &c);
        assert_eq!(messages(&d), ["`in_scope/2` needs argument 2 bound before the call"]);
        assert_eq!(d[0].predicate.as_deref(), Some("v/1"));

        // Bound later in the order than it is written: fine.
        let p = program(vec![rule(
            "v",
            vec![v("x")],
            vec![
                pos("in_scope", vec![v("x"), v("p")]),
                pos("node", vec![v("x"), Term::wild()]),
                Literal::cmp(CmpOp::Eq, v("p"), s("cli")),
            ],
        )]);
        assert!(analyse(&p, &c).is_ok());
        // Both string builtins' columns are Must.
        let p = program(vec![rule(
            "v",
            vec![v("x")],
            vec![
                pos("node", vec![v("x"), Term::wild()]),
                pos("starts_with", vec![v("q"), s("cli")]),
            ],
        )]);
        assert_eq!(
            messages(&errors_in(&p, &c)),
            ["`starts_with/2` needs argument 1 bound before the call"]
        );
    }

    #[test]
    fn aggregates() {
        let fan = |extra: bool| {
            let mut rules = vec![Rule::new(
                "fan",
                vec![v("y").into(), HeadArg::agg(AggOp::Count, &["x"])],
                vec![pos("edge", vec![v("x"), v("y"), s("CALLS")])],
            )];
            if extra {
                rules.push(rule(
                    "fan",
                    vec![v("y"), Term::int(0)],
                    vec![pos("node", vec![v("y"), Term::wild()])],
                ));
            }
            program(rules)
        };
        let a = ok(&fan(false));
        let r = &a.rules[0];
        assert_eq!(r.head_terms, vec![CTerm::Var(0), CTerm::Wild]);
        assert_eq!(r.aggs, vec![(AggOp::Count, vec![1])]);
        assert_eq!(r.var_names, ["y", "x"]);
        assert_eq!(a.strata, vec![Stratum { preds: vec![0], recursive: false, aggregate: true }]);
        assert_eq!(
            messages(&errors(&fan(true))),
            ["`fan/2` has an aggregate head, so it must have exactly one rule (found 2)"]
        );
        let unbound = program(vec![Rule::new(
            "fan",
            vec![v("y").into(), HeadArg::agg(AggOp::Max, &["z"])],
            vec![pos("edge", vec![v("x"), v("y"), s("CALLS")])],
        )]);
        assert_eq!(
            messages(&errors(&unbound)),
            ["aggregated variable `z` of `fan/2` is not bound by the body"]
        );
    }

    #[test]
    fn wildcards_arity_and_aggregate_variables() {
        let c = catalog().with_builtins(vec![in_scope()]);
        let node = || pos("node", vec![v("x"), Term::wild()]);
        // `_` in a comparison, and in a negated builtin's Must column.
        let p = program(vec![
            rule("v", vec![v("x")], vec![node(), Literal::cmp(CmpOp::Ne, v("x"), Term::wild())]),
            rule("w", vec![v("x")], vec![node(), neg("starts_with", vec![Term::wild(), s("n")])]),
        ]);
        assert_eq!(
            messages(&errors_in(&p, &c)),
            [
                "`_` may not appear in a comparison",
                "`starts_with/2` needs argument 1 bound before the call",
            ]
        );
        // In a negated relation atom, or a negated builtin's May column, `_` is a wildcard.
        let p = program(vec![rule(
            "v",
            vec![v("x")],
            vec![
                node(),
                neg("edge", vec![v("x"), Term::wild(), s("CALLS")]),
                neg("in_scope", vec![Term::wild(), s("cli")]),
            ],
        )]);
        assert!(analyse(&p, &c).is_ok());

        // Arity: 256 arguments fit a u8 column; 257 do not, for a head or a domain call.
        let fact = |n: usize| program(vec![Rule::fact("wide", vec![Term::int(0); n])]);
        assert!(analyse(&fact(MAX_ARITY), &c).is_ok());
        let d = errors(&fact(MAX_ARITY + 1));
        assert_eq!(messages(&d), ["a predicate may have at most 256 arguments; `wide` has 257"]);
        assert_eq!(d[0].predicate.as_deref(), Some("wide/257"));
        let wide = BuiltinDecl {
            name: "wide_call".into(),
            cols: vec![ColType::Str; MAX_ARITY + 1],
            bind: vec![BindReq::May; MAX_ARITY + 1],
            tiered: false,
            kind: BuiltinKind::Domain,
        };
        let cw = catalog().with_builtins(vec![wide]);
        let call = pos("wide_call", vec![Term::wild(); MAX_ARITY + 1]);
        let p = program(vec![rule("v", vec![v("x")], vec![node(), call])]);
        assert_eq!(
            messages(&errors_in(&p, &cw)),
            ["a predicate may have at most 256 arguments; `wide_call` has 257"]
        );

        // Aggregate variables: count takes one or more, min / max / sum exactly one. A
        // `count()` cannot be written (the parser wants a variable), only built.
        let agg = |op: AggOp, vars: &[&str]| {
            program(vec![Rule::new(
                "s",
                vec![v("x").into(), HeadArg::agg(op, vars)],
                vec![
                    pos("edge", vec![v("x"), v("a"), s("CALLS")]),
                    pos("edge", vec![v("x"), v("b"), s("USES")]),
                ],
            )])
        };
        let texts = |op: AggOp, vars: &[&str]| -> Vec<String> {
            errors(&agg(op, vars)).into_iter().map(|d| d.message).collect()
        };
        assert_eq!(texts(AggOp::Count, &[]), ["`count` takes at least one variable"]);
        assert_eq!(texts(AggOp::Sum, &["a", "b"]), ["`sum` takes one variable, found 2"]);
        assert_eq!(texts(AggOp::Min, &[]), ["`min` takes one variable, found 0"]);
        assert!(analyse(&agg(AggOp::Count, &["a", "b"]), &c).is_ok());
        assert!(analyse(&agg(AggOp::Max, &["a"]), &c).is_ok());
    }

    #[test]
    fn unstratifiable() {
        let mut negated = Atom::new("q", Vec::new());
        negated.span = at(10);
        let p = program(vec![
            rule("p", Vec::new(), vec![Literal::Neg(negated)]),
            rule("q", Vec::new(), vec![pos("p", Vec::new())]),
        ]);
        let d = errors(&p);
        assert_eq!(
            messages(&d),
            [
                "negation inside recursion: `p/0`, `q/0` depend on each other through this negated atom"
            ]
        );
        assert_eq!(d[0].span, at(10));
        assert_eq!(d[0].predicate.as_deref(), Some("p/0"));

        let mut agg = Rule::new(
            "p",
            vec![v("x").into(), HeadArg::agg(AggOp::Count, &["y"])],
            vec![pos("p", vec![v("y"), v("x")])],
        );
        if let HeadArg::Agg(a) = &mut agg.head.args[1] {
            a.span = at(4);
        }
        let d = errors(&program(vec![agg]));
        assert_eq!(
            messages(&d),
            ["aggregation inside recursion: `p/2` depends on itself through this aggregate"]
        );
        assert_eq!(d[0].span, at(4));

        // The names are sorted by name, not by definition order.
        let p = program(vec![
            rule(
                "zz",
                vec![v("x")],
                vec![pos("node", vec![v("x"), Term::wild()]), neg("aa", vec![v("x")])],
            ),
            rule("aa", vec![v("x")], vec![pos("zz", vec![v("x")])]),
        ]);
        assert_eq!(
            messages(&errors(&p)),
            [
                "negation inside recursion: `aa/1`, `zz/1` depend on each other through this negated atom"
            ]
        );
    }

    #[test]
    fn synthetic_names() {
        let p = program(vec![
            rule(
                "__a_tc__bf",
                vec![v("x"), v("y")],
                vec![pos("edge", vec![v("x"), v("y"), s("CALLS")])],
            ),
            rule(
                "out",
                vec![v("y")],
                vec![
                    pos("__a_tc__bf", vec![v("x"), v("y")]),
                    pos("node", vec![v("x"), s("ROUTE")]),
                ],
            ),
        ]);
        let d = errors(&p);
        assert_eq!(messages(&d), ["predicate names starting `__` are reserved"]);
        assert_eq!(d[0].predicate.as_deref(), Some("__a_tc__bf/2"));
        let a = match analyse_with(&p, &catalog(), true) {
            Ok(a) => a,
            Err(e) => panic!("{e}"),
        };
        assert!(a.preds[id_of(&a, "__a_tc__bf") as usize].synthetic);
        assert!(!a.preds[id_of(&a, "out") as usize].synthetic);
        assert_eq!(a.predicates(), ["out/1"]);
    }

    #[test]
    fn strata_order() {
        let a_rule = || rule("a", vec![v("x")], vec![pos("node", vec![v("x"), s("ROUTE")])]);
        let b_rule =
            || rule("b", vec![v("x")], vec![pos("edge", vec![v("x"), Term::wild(), s("CALLS")])]);
        let c_rule =
            || rule("c", vec![v("x")], vec![pos("a", vec![v("x")]), pos("b", vec![v("x")])]);
        let an = ok(&program(vec![a_rule(), b_rule(), c_rule()]));
        assert_eq!(an.predicates(), ["a/1", "b/1", "c/1"]);
        assert_eq!(an.strata_count(), 3);
        assert_eq!((stratum_of(&an, "a"), stratum_of(&an, "b"), stratum_of(&an, "c")), (0, 1, 2));
        // EDB predicates follow the IDB ones, in first-reference order.
        let edb: Vec<(String, PredKind)> =
            an.preds[3..].iter().map(|p| (p.signature(), p.kind)).collect();
        assert_eq!(
            edb,
            [
                ("node/2".to_string(), PredKind::Edb(EdbPred::Node)),
                ("edge/3".to_string(), PredKind::Edb(EdbPred::Edge))
            ]
        );

        // Every order of the three rules: the dependent one after both, the two
        // independent ones in source order.
        let orders = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
        let make: [fn() -> Rule; 3] = [a_rule, b_rule, c_rule];
        for order in orders {
            let an = ok(&program(order.iter().map(|&i| make[i]()).collect()));
            let (sa, sb, sc) = (stratum_of(&an, "a"), stratum_of(&an, "b"), stratum_of(&an, "c"));
            assert!(sa < sc && sb < sc, "{order:?}");
            let a_first = order.iter().position(|&i| i == 0) < order.iter().position(|&i| i == 1);
            assert_eq!(sa < sb, a_first, "{order:?}");
        }

        // A recursive pair is one recursive stratum.
        let an = ok(&program(tc(vec![pos("edge", vec![v("x"), v("y"), s("CALLS")])])));
        assert_eq!(an.strata, vec![Stratum { preds: vec![0], recursive: true, aggregate: false }]);
        assert!(an.preds[0].recursive);
    }

    #[test]
    fn warnings() {
        let a = ok(&program(tc(vec![pos("edge", vec![v("x"), v("y"), s("CALLS")])])));
        assert_eq!(
            messages(&a.warnings),
            [
                "recursive predicate `tc/2` is unseeded: its recursion starts from every row of `edge/3`; \
              seed it (`tc(x, y) :- seed(x), ...`) or use reaches(x, y, \"CATS\"); magic sets seed it \
              when every caller binds an argument"
            ]
        );
        assert_eq!(a.warnings[0].severity, Severity::Warning);
        assert_eq!(a.warnings[0].predicate.as_deref(), Some("tc/2"));

        // Seeded by a lower IDB predicate, a constant, or a builtin's Must constant.
        let mut seeded =
            vec![rule("seed", vec![v("x")], vec![pos("node", vec![v("x"), Term::wild()])])];
        seeded.extend(tc(vec![
            pos("seed", vec![v("x")]),
            pos("edge", vec![v("x"), v("y"), s("CALLS")]),
        ]));
        assert!(ok(&program(seeded)).warnings.is_empty());
        let by_kind = tc(vec![
            pos("node", vec![v("x"), s("ROUTE")]),
            pos("edge", vec![v("x"), v("y"), s("CALLS")]),
        ]);
        assert!(ok(&program(by_kind)).warnings.is_empty());
        let c = catalog().with_builtins(vec![in_scope()]);
        let by_scope = tc(vec![
            pos("in_scope", vec![v("x"), s("cli")]),
            pos("edge", vec![v("x"), v("y"), s("CALLS")]),
        ]);
        assert!(ok_in(&program(by_scope), &c).warnings.is_empty());
        // A cell type constant picks a family, so it does not seed.
        let by_cell = tc(vec![
            pos("cell", vec![v("x"), s("CODE"), Term::wild()]),
            pos("edge", vec![v("x"), v("y"), s("CALLS")]),
        ]);
        let w = ok(&program(by_cell)).warnings;
        assert_eq!(w.len(), 1);
        assert!(w[0].message.contains("every row of `cell/3`"), "{}", w[0].message);

        let a = ok(&program(vec![rule(
            "v",
            vec![v("x")],
            vec![pos("edge", vec![v("x"), v("y"), s("CALLS")])],
        )]));
        assert_eq!(messages(&a.warnings), ["variable `y` appears once; write `_`"]);
        assert_eq!(a.warnings[0].predicate.as_deref(), Some("v/1"));
        // A leading underscore marks a variable as meant to be unused.
        let a = ok(&program(vec![rule(
            "v",
            vec![v("x")],
            vec![pos("edge", vec![v("x"), v("_y"), s("CALLS")])],
        )]));
        assert!(a.warnings.is_empty());
        assert_eq!(example_head("wide", 6), "wide(x, y, z, w, x5, x6)");
        assert_eq!(example_head("flag", 0), "flag");
    }

    #[test]
    fn negated_reads() {
        let p = program(vec![
            rule(
                "handled",
                vec![v("r")],
                vec![pos("edge", vec![v("r"), Term::wild(), s("HANDLED_BY")])],
            ),
            rule(
                "v",
                vec![v("r")],
                vec![pos("node", vec![v("r"), s("ROUTE")]), neg("handled", vec![v("r")])],
            ),
        ]);
        let a = ok(&p);
        assert!(a.rules[0].negated_reads.is_empty());
        assert_eq!(
            a.rules[1].negated_reads,
            vec![NegRead {
                negated: "handled/1".into(),
                categories: vec!["HANDLED_BY".into()],
                any_category: false
            }]
        );

        // The cone follows the predicates read, through the carry set and `*`; a
        // variable category reads any category; a negated EDB atom reads its own.
        let p = program(vec![
            rule(
                "live",
                vec![v("y")],
                vec![pos("node", vec![v("x"), s("ROUTE")]), pos("reaches", vec![v("x"), v("y")])],
            ),
            rule(
                "imp",
                vec![v("y")],
                vec![pos("edge", vec![v("x"), v("y"), s("IMPORTS")]), pos("live", vec![v("x")])],
            ),
            rule(
                "anyc",
                vec![v("x")],
                vec![
                    pos("edge", vec![v("x"), Term::wild(), v("c")]),
                    pos("category", vec![v("c")]),
                ],
            ),
            rule(
                "all",
                vec![v("x")],
                vec![
                    pos("node", vec![v("x"), Term::wild()]),
                    pos("scc", vec![v("x"), Term::wild(), s("*")]),
                ],
            ),
            rule(
                "dead",
                vec![v("f")],
                vec![
                    pos("node", vec![v("f"), s("FUNCTION")]),
                    neg("imp", vec![v("f")]),
                    neg("anyc", vec![v("f")]),
                    neg("all", vec![v("f")]),
                    neg("edge", vec![v("f"), Term::wild(), s("USES")]),
                ],
            ),
        ]);
        let a = ok(&p);
        let reads: Vec<(String, Vec<String>, bool)> = a.rules[4]
            .negated_reads
            .iter()
            .map(|n| (n.negated.clone(), n.categories.clone(), n.any_category))
            .collect();
        let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            reads,
            [
                ("imp/1".to_string(), strs(&["CALLS", "IMPORTS", "USES"]), false),
                ("anyc/1".to_string(), Vec::new(), true),
                ("all/1".to_string(), Vec::new(), true),
                ("edge/3".to_string(), strs(&["USES"]), false),
            ]
        );
    }

    #[test]
    fn sips_bound_first() {
        let c = catalog().with_builtins(vec![in_scope()]);
        let p = program(vec![rule(
            "v",
            vec![v("b")],
            vec![
                pos("edge", vec![v("a"), v("b"), v("c")]),
                pos("carry_category", vec![v("c")]),
                pos("in_scope", vec![v("a"), s("cli")]),
            ],
        )]);
        let a = ok_in(&p, &c);
        let r = &a.rules[0];
        assert_eq!(sips_order(r, &a.preds, &c, &[], None, &|_| None), vec![2, 0, 1]);
        // `first` runs first; then edge (one bound column, c) ties in_scope (its
        // constant) and wins by source order, and in_scope is left a filter.
        assert_eq!(sips_order(r, &a.preds, &c, &[], Some(1), &|_| None), vec![1, 0, 2]);
        // Pre-bound head variable b (the explain plan): edge has a bound column, so it
        // ties in_scope at one and runs first by source order.
        let b = r.var_names.iter().position(|n| n == "b").unwrap_or(usize::MAX);
        let mut bound = vec![false; usize::from(r.nvars)];
        bound[b] = true;
        assert_eq!(sips_order(r, &a.preds, &c, &bound, None, &|_| None), vec![0, 1, 2]);

        // Ties to the smaller known size, then source order; filters run as soon as
        // they are fully bound, assignments when one side is.
        let p = program(vec![rule(
            "w",
            vec![v("x"), v("y")],
            vec![
                pos("node", vec![v("x"), Term::wild()]),
                pos("edge", vec![v("x"), v("y"), s("CALLS")]),
                Literal::cmp(CmpOp::Ne, v("x"), v("y")),
                Literal::Cmp(Comparison {
                    op: CmpOp::Eq,
                    left: v("z"),
                    right: v("y"),
                    span: Span::default(),
                }),
                pos("node", vec![v("z"), s("ROUTE")]),
            ],
        )]);
        let a = ok(&p);
        let r = &a.rules[0];
        let edge = id_of(&a, "edge");
        // Unknown sizes: edge and node(z, "ROUTE") tie at one bound column, edge first
        // by source order; node(x, _) and `x != y` are then filters; the assignment
        // (size 1) beats node(z, "ROUTE"), which it leaves a filter.
        assert_eq!(sips_order(r, &a.preds, &c, &[], None, &|_| None), vec![1, 0, 2, 3, 4]);
        let small_edge = |p: PredId| Some(if p == edge { 10 } else { 1_000 });
        assert_eq!(sips_order(r, &a.preds, &c, &[], None, &small_edge), vec![1, 0, 2, 3, 4]);
        // A smaller node relation wins the tie: node(z, "ROUTE") first, then `z = y`
        // (size 1 beats edge's 1,000), then edge with y bound, then the filters.
        let small_node = |p: PredId| Some(if p == edge { 1_000 } else { 10 });
        assert_eq!(sips_order(r, &a.preds, &c, &[], None, &small_node), vec![4, 3, 1, 0, 2]);
    }

    #[test]
    fn sips_joins_before_a_constant_only_literal() {
        // DA.17's case: `seg(b, "engine")` holds a constant but shares no bound
        // variable once `seg(a, "parsers")` ran, so ranking it beside the join on `a`
        // by its constant (and its smaller size) ran it as a cross product of the two
        // scopes. The join runs first and leaves it a filter.
        let c = catalog().with_attrs(&[crate::ir::AttrDecl { name: "seg", ty: ColType::Str }]);
        let p = program(vec![
            rule(
                "f",
                vec![v("a"), v("b")],
                vec![
                    pos("seg", vec![v("a"), s("parsers")]),
                    pos("edge", vec![v("a"), v("b"), v("c")]),
                    pos("carry_category", vec![v("c")]),
                    pos("seg", vec![v("b"), s("engine")]),
                ],
            ),
            // The same filter through an assignment: `s = "engine"` binds s, so
            // seg(b, s) joins on it, and edge then joins on b.
            rule(
                "g",
                vec![v("a"), v("b")],
                vec![
                    pos("seg", vec![v("a"), s("parsers")]),
                    pos("edge", vec![v("a"), v("b"), v("c")]),
                    pos("carry_category", vec![v("c")]),
                    pos("seg", vec![v("b"), v("s")]),
                    Literal::cmp(CmpOp::Eq, v("s"), s("engine")),
                ],
            ),
        ]);
        let a = ok_in(&p, &c);
        let (seg, edge) = (id_of(&a, "seg"), id_of(&a, "edge"));
        let size = |q: PredId| {
            Some(match q {
                q if q == seg => 2_000,
                q if q == edge => 60_000,
                _ => 2,
            })
        };
        assert_eq!(sips_order(&a.rules[0], &a.preds, &c, &[], None, &size), vec![0, 1, 2, 3]);
        assert_eq!(sips_order(&a.rules[1], &a.preds, &c, &[], None, &size), vec![4, 3, 1, 0, 2]);
        // With nothing joined yet the constants still rank: seg(a, ..) and seg(b, ..)
        // tie and source order starts from seg(a, ..).
        assert_eq!(sips_order(&a.rules[0], &a.preds, &c, &[], None, &|_| None)[0], 0);
    }

    /// `in_seg(Node, Str)`: the builtin form of the `seg` attribute (node `i`'s
    /// segment is `s<i % 10>`), memoised per node through `Lookup`.
    struct InSeg {
        order: Vec<glia_core::NodeId>,
        index: crate::value::Lookup<glia_core::NodeId, u32>,
    }

    fn seg_of(i: u32) -> String {
        format!("s{}", i % 10)
    }

    impl crate::source::Builtin for InSeg {
        fn decl(&self) -> BuiltinDecl {
            BuiltinDecl {
                name: "in_seg".into(),
                cols: vec![ColType::Node, ColType::Str],
                bind: vec![BindReq::May, BindReq::Must],
                tiered: false,
                kind: BuiltinKind::Domain,
            }
        }

        fn call(
            &self,
            args: &[Option<crate::value::ValueRef<'_>>],
            out: &mut Vec<crate::source::BuiltinRow>,
        ) -> Result<(), String> {
            use crate::value::{Tier, Value, ValueRef};
            let [node, Some(ValueRef::Str(scope))] = args else {
                return Err("in_seg takes (node, bound scope)".to_string());
            };
            let row = |id: glia_core::NodeId| crate::source::BuiltinRow {
                values: vec![Value::Node(id), Value::Str((*scope).to_string())],
                tier: Tier::Fact,
            };
            match node {
                Some(ValueRef::Node(id)) => {
                    if self.index.get(id).is_some_and(|i| seg_of(*i) == *scope) {
                        out.push(row(*id));
                    }
                }
                Some(_) => {}
                None => {
                    for (i, id) in self.order.iter().enumerate() {
                        if seg_of(i as u32) == *scope {
                            out.push(row(*id));
                        }
                    }
                }
            }
            Ok(())
        }
    }

    #[test]
    fn attribute_filter_costs_like_the_builtin() {
        // DA.17 measured `seg(x, "parsers")` about 74x slower than the builtin form
        // at x1 (and unfinished at x10): the second scope filter ran as a cross
        // product of the two scopes (sips_joins_before_a_constant_only_literal), 1,500
        // x 1,500 bindings per variant here, against about 4,500 for the join. Both
        // forms now plan the same join, so they answer alike in comparable time (best
        // of 3 each; the cross product measured about 6x the builtin form at 6,000
        // nodes, and grows with the square of a scope).
        use crate::eval::{Options, run};
        use crate::source::MemSource;
        use crate::testkit::{GraphSpec, lcg_graph, node_id};
        use crate::value::Value;
        use glia_activation::algo::GraphSource;
        use std::time::Instant;
        const N: u32 = 15_000;
        let spec = GraphSpec {
            nodes: N,
            edges: 45_000,
            categories: vec!["CALLS", "USES", "IMPORTS"],
            tier_mix: [70, 20, 10],
            seed: 7,
            salt: 1,
        };
        let mut g: MemSource = lcg_graph(&spec);
        g.attr(crate::ir::AttrDecl { name: "seg", ty: ColType::Str });
        let mut index = crate::value::Lookup::new();
        for i in 0..N {
            g.set_attr(node_id(1, i), 1, Value::Str(seg_of(i)));
            index.insert(node_id(1, i), i);
        }
        let order = g.node_ids();
        g.builtin(Box::new(InSeg { order, index }));
        let attr = "f(a, b) :- seg(a, \"s3\"), edge(a, b, c), carry_category(c), seg(b, \"s7\").";
        let builtin =
            "f(a, b) :- in_seg(a, \"s3\"), edge(a, b, c), carry_category(c), in_seg(b, \"s7\").";
        let best_of = |text: &str| {
            let mut best = f64::MAX;
            let mut rows = Vec::new();
            for _ in 0..3 {
                let t = Instant::now();
                let ev = run("rules", text, &g, &Options::default());
                let ev = ev.unwrap_or_else(|e| panic!("{e}"));
                best = best.min(t.elapsed().as_secs_f64() * 1_000.0);
                rows = ev.rows("f").unwrap_or_else(|e| panic!("{e}"));
            }
            (best, rows)
        };
        let (attr_ms, attr_rows) = best_of(attr);
        let (builtin_ms, builtin_rows) = best_of(builtin);
        let values = |rows: &[crate::eval::Row]| {
            rows.iter().map(|r| (r.values.clone(), r.tier)).collect::<Vec<_>>()
        };
        assert!(!attr_rows.is_empty());
        assert_eq!(values(&attr_rows), values(&builtin_rows));
        eprintln!("[rules-bench] attr-filter attr_ms={attr_ms:.1} builtin_ms={builtin_ms:.1}");
        assert!(
            attr_ms <= 3.0 * builtin_ms + 50.0,
            "attribute form {attr_ms:.1} ms vs builtin form {builtin_ms:.1} ms"
        );
    }

    #[test]
    fn sips_does_not_count_a_builtin_const_column() {
        // DA.10's probe: `reaches`' category set is a literal in every call, so the
        // literal that binds an end (in_scope's Must constant) runs first and the walk
        // starts from a bound node instead of every node.
        let c = catalog().with_builtins(vec![in_scope()]);
        let p = program(vec![rule(
            "hit",
            vec![v("a")],
            vec![
                pos("reaches", vec![v("a"), v("b"), s("CALLS")]),
                pos("in_scope", vec![v("b"), s("store")]),
            ],
        )]);
        let a = ok_in(&p, &c);
        let r = &a.rules[0];
        assert_eq!(sips_order(r, &a.preds, &c, &[], None, &|_| None), vec![1, 0]);
        // reaches/4's depth constant is no key either: node(a, _) and the walk tie at
        // nothing bound and source order runs node first, so the walk gets `a` bound
        // (counting the two constants ranked the walk first with both ends free).
        let p = program(vec![rule(
            "near",
            vec![v("a"), v("b")],
            vec![
                pos("node", vec![v("a"), Term::wild()]),
                pos("reaches", vec![v("a"), v("b"), s("CALLS"), Term::int(3)]),
            ],
        )]);
        let a = ok(&p);
        let r = &a.rules[0];
        assert_eq!(sips_order(r, &a.preds, &c, &[], None, &|_| None), vec![0, 1]);
    }

    #[test]
    fn analysis_shape() {
        let c = catalog().with_attrs(&[crate::ir::AttrDecl { name: "index", ty: ColType::Int }]);
        let p = program(vec![
            rule(
                "seed",
                vec![v("x")],
                vec![
                    pos("node", vec![v("x"), Term::wild()]),
                    pos("index", vec![v("x"), Term::int(0)]),
                ],
            ),
            rule(
                "hit",
                vec![v("y")],
                vec![pos("seed", vec![v("x")]), pos("reaches", vec![v("x"), v("y"), s("CALLS")])],
            ),
        ]);
        let a = ok_in(&p, &c);
        let kinds: Vec<(String, PredKind)> =
            a.preds.iter().map(|p| (p.signature(), p.kind)).collect();
        assert_eq!(
            kinds,
            [
                ("seed/1".to_string(), PredKind::Idb),
                ("hit/1".to_string(), PredKind::Idb),
                ("node/2".to_string(), PredKind::Edb(EdbPred::Node)),
                ("index/2".to_string(), PredKind::Edb(EdbPred::Attr(8))),
                ("reaches/3".to_string(), PredKind::Builtin(1)),
            ]
        );
        assert_eq!(a.rules.iter().map(|r| r.index).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(a.rules[1].body[0], CLit::Pos(0, vec![CTerm::Var(1)], Span::default()));
        assert_eq!(a.preds[3].col_type(&c, 1), ColType::Int);
        assert_eq!(a.preds[4].bind_req(&c, 2), Some(BindReq::Const));
        assert_eq!(a.preds[0].col_type(&c, 0), ColType::Any);
        assert_eq!(closure_set(&c, 0, &[CTerm::Wild, CTerm::Wild]), Some(CatSet::Carry));
        assert_eq!(closure_set(&c, 5, &[CTerm::Wild, CTerm::Wild]), None);
        assert_eq!(CatSet::Carry.names(&c), (vec!["CALLS".to_string(), "USES".to_string()], false));
        assert!(a.warnings.is_empty());
        // Analysis is a function of the text: twice gives the same compiled program.
        let b = ok_in(&p, &c);
        assert_eq!((a.preds, a.rules, a.strata), (b.preds, b.rules, b.strata));
        assert_eq!(ok(&Program::new()).strata_count(), 0);
    }
}
