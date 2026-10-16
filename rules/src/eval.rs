//! evaluate / run: lazy EDB loading, the magic rewrite with an unstratifiable fallback,
//! levelled semi-naive evaluation per stratum over exec (DA.14a), aggregate strata, the
//! row budget, absence caveats and rows sorted by rendered values (DA.14b).
//!
//! **Order of work.** [`evaluate`] analyses the program against the source's catalog
//! (its declaration conflicts become warnings), resolves the outputs, and, with
//! `Options::magic`, applies the demand rewrite (DA.12, which prints its own `[rules]
//! magic` lines) and re-analyses it, keeping the original program when that fails. It
//! then loads only the EDB predicates the program names, each once, narrowed to the
//! categories / cell types every atom of the family names, and runs the strata lowest
//! first, each planned just before it runs (DA.10) so the sizes it ranks by are real.
//!
//! **The tier lattice, evaluated exactly.** A row's tier is the min over its
//! derivations of the max over their inputs. Each rule stratum runs its fixpoint three
//! times, incrementally: with only FACT inputs visible, then adding DERIVED, then
//! HEURISTIC. A row is inserted once, in the level whose fixpoint first derives it, so
//! its level IS that min-of-max and no row's tier is ever updated. Which variants run
//! (the Src -> View mapping is exec's, stated in its module doc):
//! - level FACT, round 1: only `delta: None` (every literal All), since Old - the
//!   levels below FACT - is empty;
//! - level L > FACT, round 1: the variants whose delta literal has level-L rows: an
//!   EDB or lower-stratum atom with any (counted per predicate as rows land), or a
//!   tiered builtin (reaches, scc, the hoisted composite, a tiered domain builtin);
//!   the stratum's own predicates hold no level-L rows yet;
//! - every later round: the variants whose delta is one of the stratum's own atoms
//!   with recent rows (complete input has no recent run).
//!
//! A non-recursive stratum runs one round per level. A level whose first round has
//! nothing to run is skipped. Negations read the complete relation and add no tier
//! (Q5); a row carries the static negation cones of its recorded rule instead.
//!
//! **Minimal heights.** A row's recorded (rule, height) is its lowest-height
//! derivation at its level, each lower-level row read at its own recorded height;
//! explain (DA.15) searches below that height. A level's first round can derive a row
//! from a long lower-level prefix before a later round finds a shorter derivation at
//! that level (DA.15's counterexample), so a recursive stratum holds candidates back by
//! height: each round inserts the candidates, new and held, at most one above the
//! lowest height among them, and holds the rest. Any derivation not yet made reads a
//! row inserted in this round or later, of height at least that lowest, so it is at
//! least one above it and no inserted row can lose to it. The level ends when nothing
//! is held and a round inserts nothing. A held candidate is one exec emitted, so the
//! budget is unchanged; the extra rounds count in `rounds`. One height per row is kept,
//! not one per level: a lower-level row is read at its own level's height even when
//! this level reaches it in fewer steps (a FACT `tc(x, p)` of 7 hops that DERIVED edges
//! reach in 3), so a row derived through it records the longer height, and its witness
//! is a valid path of its tier but not always a shortest one.
//!
//! **Aggregates** run their one body over every level at once and hand the projected
//! bindings to `aggregate::aggregate`, ordered by `ValueTable::cmp_rendered`.
//!
//! **Budget.** exec counts emitted bindings and builtin / closure rows; this module adds
//! every inserted IDB and aggregate row. Past `Options::max_rows` evaluation stops with
//! `RulesError::Budget` naming the head being derived (a magic copy names its user
//! predicate) and prints `[rules] budget pred=<name>/<arity> rows=<n> cap=<cap>`.
//!
//! **Markers**, once per evaluation, on stderr: `[rules] eval strata=<n> rules=<r>
//! variants=<v> edb=<rows> derived=<rows> rounds=<k> aggregates=<a> magic=<m>
//! values=<h> warnings=<w> ms=<t>`, then one `[rules] closure cats=<..> mode=<..>
//! bfs=<n> rows=<r>` per category set a closure builtin walked.
//!
//! **Answers.** [`Evaluation::rows`] answers the outputs only: magic keeps the answers
//! of the predicates that read a rewritten one, never the rewritten predicate's full
//! extension, and outputs are never rewritten. Rows sort by rendered values column by
//! column (type rank, then the value, a node by `render_node`), never by handle or id.
//! Single-threaded; every map is a `value::Lookup`.
//!
//! **Kept for explain (DA.15).** `Evaluation`'s fields are crate-visible so explain.rs
//! reads them. The closure walks cannot be kept: `Closure` borrows the edge tiers it is
//! built over, so it lives only for the evaluation and explain builds its own over
//! `tiers`; the counters it reported stay in `closure_stats`.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ops::RangeInclusive;
use std::time::Instant;

use glia_activation::profile::Registries;
use glia_core::{CellTypeId, EdgeCategoryId, NodeId};

use crate::aggregate;
use crate::analysis::{
    Analysis, CLit, CRule, CTerm, CatSet, ConstVal, EdbPred, NegRead, PredId, PredKind, Stratum,
    analyse, analyse_with,
};
use crate::closure::{Closure, ClosureStats};
use crate::diag::{Diagnostic, RulesError, Span};
use crate::exec::{Binding, Budget, Ctx, Memo, Mode, MultiState, run_body};
use crate::ir::{BuiltinKind, Catalog, Program};
use crate::magic;
use crate::parser::parse_program;
use crate::plan::{KeyPart, Step, StratumPlan, plan_stratum};
use crate::relation::{EDB_RULE, Relation, RowMeta, View, Which};
use crate::source::{Declared, EdbRows, FactSource, Restrict, declare, edge_tiers, load_edb};
use crate::value::{Handle, Tier, Value, ValueRef, ValueTable};

/// The default row budget: 5,000,000 rows. The R1 re-run at x10 of HEAD 22c65ee
/// (257,680 nodes / 826,755 edges) stops the unseeded carry closure (13,551,390 rows)
/// and CALLS closure (7,840,842) while the largest seeded, check-shaped relation (the
/// pairwise forbid, 704,796 rows) keeps 7x headroom.
pub const DEFAULT_MAX_ROWS: usize = 5_000_000;

/// An unbound variable in a binding: exec's `FREE`, the one handle no value gets.
const UNBOUND: Handle = Handle::MAX;

/// Every level, stable and recent: what an answer reads.
const EVERY_ROW: RangeInclusive<u8> = 0..=2;

/// How [`evaluate`] runs. Built with `Default` and field assignment outside the crate.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Options {
    /// The row budget ([`DEFAULT_MAX_ROWS`]).
    pub max_rows: usize,
    /// The predicates [`Evaluation::rows`] answers, as `name` or `name/arity`. Empty:
    /// every user IDB predicate no rule of ANOTHER predicate reads (a recursive
    /// predicate reading itself still counts).
    pub outputs: Vec<String>,
    /// Apply the magic-sets rewrite to unseeded recursion read from bound sites.
    pub magic: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { max_rows: DEFAULT_MAX_ROWS, outputs: Vec::new(), magic: true }
    }
}

/// One answer row: its values in column order, its tier, and the absence caveats of
/// the negations it depends on (Q5).
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Row {
    pub values: Vec<Value>,
    pub tier: Tier,
    pub absence: Vec<AbsenceCaveat>,
    pub(crate) at: RowAt,
}

/// Where a row lives, for explain: the predicate (an evaluated-program id), its level,
/// its provenance and its handles in column order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RowAt {
    pub(crate) pred: PredId,
    pub(crate) level: Tier,
    pub(crate) meta: RowMeta,
    pub(crate) row: Vec<Handle>,
}

/// A negated atom a row depends on: its predicate and the edge categories its absence
/// reads (a static cone, so an over-approximation by design).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct AbsenceCaveat {
    /// `name/arity`.
    pub negated: String,
    /// The constant category names read, sorted.
    pub categories: Vec<String>,
    /// Some read names no constant category (`"*"`, a variable or `_`).
    pub any_category: bool,
}

/// One evaluation's counters (the `[rules] eval` marker's fields).
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct EvalStats {
    pub strata: usize,
    pub rules: usize,
    pub variants: usize,
    pub edb_rows: usize,
    /// IDB and aggregate rows inserted.
    pub derived_rows: usize,
    pub rounds: usize,
    pub aggregates: usize,
    /// Distinct predicates the magic rewrite adorned (0 when it kept the program).
    pub magic_rewrites: usize,
    /// Distinct values interned.
    pub values: usize,
    pub elapsed_ms: f64,
}

/// A finished evaluation: the rows of every stratum and what explain (DA.15) reads.
pub struct Evaluation<'s, S: ?Sized> {
    pub(crate) src: &'s S,
    /// The catalog and the builtin implementations behind it.
    pub(crate) declared: Declared<'s>,
    /// The program evaluated: the magic rewrite's output, or the input.
    pub(crate) program: Program,
    pub(crate) analysis: Analysis,
    pub(crate) values: ValueTable,
    /// By `PredId` of `analysis`; a builtin's is empty.
    pub(crate) relations: Vec<Relation>,
    /// One `(x, rep)` relation per scc category set.
    pub(crate) scc: Vec<(CatSet, Relation)>,
    /// Every edge's tier by ordinal (empty when no edge or closure was read).
    pub(crate) tiers: Vec<Tier>,
    pub(crate) closure_stats: ClosureStats,
    /// exec's builtin rows.
    pub(crate) memo: Memo,
    /// Per stratum: its plan (indexes resolved) and exec's ReachMulti states by slot.
    pub(crate) plans: Vec<StratumPlan>,
    pub(crate) multi: Vec<Vec<MultiState>>,
    /// By `CRule::index`: the absence caveats of a row that rule derived.
    pub(crate) caveats: Vec<Vec<AbsenceCaveat>>,
    /// The outputs, sorted by signature.
    pub(crate) outputs: Vec<PredId>,
    pub(crate) max_rows: usize,
    pub(crate) stats: EvalStats,
    pub(crate) warnings: Vec<Diagnostic>,
}

/// Parse `text` as the source `source_name` and [`evaluate`] it.
pub fn run<'s, S: FactSource + ?Sized>(
    source_name: &str,
    text: &str,
    src: &'s S,
    opts: &Options,
) -> Result<Evaluation<'s, S>, RulesError> {
    evaluate(&parse_program(source_name, text)?, src, opts)
}

/// Evaluate `program` over `src` (module doc).
pub fn evaluate<'s, S: FactSource + ?Sized>(
    program: &Program,
    src: &'s S,
    opts: &Options,
) -> Result<Evaluation<'s, S>, RulesError> {
    let started = Instant::now();
    let declared = declare(src);
    let original = analyse(program, &declared.catalog)?;
    let source = program.source_name(0);
    let mut warnings: Vec<Diagnostic> = declared
        .conflicts
        .iter()
        .map(|m| Diagnostic::warning(Span::default(), source, None, m.clone()))
        .collect();
    warnings.extend(original.warnings.iter().cloned());
    let wanted = resolve_outputs(&original, &opts.outputs)?;
    let wanted_sigs: Vec<(String, usize)> = wanted
        .iter()
        .map(|&p| (original.preds[p as usize].name.clone(), original.preds[p as usize].arity))
        .collect();
    let (evaluated, an, magic_rewrites) = if opts.magic {
        demand(program, original, &declared.catalog, &wanted)
    } else {
        (program.clone(), original, 0)
    };
    if an.rules.len() > usize::from(EDB_RULE) {
        let message =
            format!("a program may hold at most {} rules (found {})", EDB_RULE, an.rules.len());
        return Err(RulesError::Invalid(vec![Diagnostic::error(
            Span::default(),
            source,
            None,
            message,
        )]));
    }
    let mut outputs: Vec<PredId> = wanted_sigs
        .iter()
        .filter_map(|(name, arity)| {
            an.preds.iter().position(|p| {
                p.kind == PredKind::Idb && !p.synthetic && &p.name == name && p.arity == *arity
            })
        })
        .map(|p| p as PredId)
        .collect();
    outputs.sort_by_key(|&p| an.preds[p as usize].signature());

    let tiers = if reads_edges(&an, &declared.catalog) { edge_tiers(src) } else { Vec::new() };
    let mut st = State {
        src,
        values: ValueTable::new(),
        rels: an.preds.iter().map(|p| Relation::new(p.arity, &[])).collect(),
        at_level: vec![[0; 3]; an.preds.len()],
        scc: Vec::new(),
        closure: Closure::new(src, &tiers),
        memo: Memo::new(),
        budget: Budget::new(opts.max_rows),
        stats: EvalStats { magic_rewrites, ..EvalStats::default() },
    };
    st.load(&an, &declared.catalog, &tiers);
    let mut plans = Vec::with_capacity(an.strata.len());
    let mut multi = Vec::with_capacity(an.strata.len());
    for k in 0..an.strata.len() {
        let (plan, states) = st.stratum(&an, &declared, k, program)?;
        plans.push(plan);
        multi.push(states);
    }
    let State { values, rels, scc, closure, memo, mut stats, .. } = st;
    let closure_stats = closure.stats().clone();
    drop(closure);
    stats.strata = an.strata.len();
    stats.rules = an.rules.len();
    stats.values = values.len();
    let caveats = rule_caveats(&an);
    stats.elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let ev = Evaluation {
        src,
        declared,
        program: evaluated,
        analysis: an,
        values,
        relations: rels,
        scc,
        tiers,
        closure_stats,
        memo,
        plans,
        multi,
        caveats,
        outputs,
        max_rows: opts.max_rows,
        stats,
        warnings,
    };
    eprintln!("{}", eval_line(&ev.stats, ev.warnings.len()));
    for line in closure_lines(&ev.closure_stats, &ev.declared.catalog.registries) {
        eprintln!("{line}");
    }
    Ok(ev)
}

impl<'s, S: FactSource + ?Sized> Evaluation<'s, S> {
    /// The outputs as `name/arity`, sorted.
    pub fn outputs(&self) -> Vec<String> {
        self.outputs.iter().map(|&p| self.analysis.preds[p as usize].signature()).collect()
    }

    /// The rows of output `pred` (`name` or `name/arity`), sorted by rendered values
    /// column by column. `Unknown` for any other name, listing the outputs.
    pub fn rows(&self, pred: &str) -> Result<Vec<Row>, RulesError> {
        let found = self.outputs.iter().copied().find(|&p| {
            let info = &self.analysis.preds[p as usize];
            info.name == pred || info.signature() == pred
        });
        let Some(p) = found else {
            return Err(RulesError::Unknown { predicate: pred.to_string(), known: self.outputs() });
        };
        let mut raw: Vec<(Vec<Handle>, Tier, RowMeta)> = Vec::new();
        if let Some(rel) = self.relations.get(p as usize) {
            let view = View { levels: EVERY_ROW, which: Which::Both, max_height: None };
            rel.scan(0, view, &mut |r, t, m, _| raw.push((r.to_vec(), t, m)));
        }
        let keys = Keys::new(self.src, &self.values, raw.iter().flat_map(|r| r.0.iter().copied()));
        raw.sort_by(|a, b| {
            a.0.iter()
                .zip(&b.0)
                .map(|(x, y)| keys.cmp(*x, *y))
                .find(|o| o.is_ne())
                .unwrap_or(Ordering::Equal)
        });
        // An aggregate row is loaded, so it names no rule: its predicate has exactly one.
        let only = self.analysis.rules.iter().find(|r| r.head == p).map(|r| r.index);
        Ok(raw
            .into_iter()
            .map(|(row, tier, meta)| {
                let rule = if meta.rule == EDB_RULE { only } else { Some(usize::from(meta.rule)) };
                Row {
                    values: row.iter().map(|h| self.values.get(*h).to_owned()).collect(),
                    tier,
                    absence: rule.and_then(|r| self.caveats.get(r)).cloned().unwrap_or_default(),
                    at: RowAt { pred: p, level: tier, meta, row },
                }
            })
            .collect())
    }

    /// A value as rows sort by it: a node's `render_node`, a string as is, an int in
    /// decimal.
    pub fn render(&self, v: &Value) -> String {
        match v {
            Value::Node(id) => self.src.render_node(*id),
            Value::Str(s) => s.clone(),
            Value::Int(i) => i.to_string(),
        }
    }

    pub fn stats(&self) -> &EvalStats {
        &self.stats
    }

    /// The catalog's declaration conflicts, then the original program's analysis
    /// warnings (never the rewrite's).
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }
}

/// The `[rules] eval` marker line (ms truncated to one decimal).
pub(crate) fn eval_line(s: &EvalStats, warnings: usize) -> String {
    format!(
        "[rules] eval strata={} rules={} variants={} edb={} derived={} rounds={} aggregates={} \
         magic={} values={} warnings={} ms={:.1}",
        s.strata,
        s.rules,
        s.variants,
        s.edb_rows,
        s.derived_rows,
        s.rounds,
        s.aggregates,
        s.magic_rewrites,
        s.values,
        warnings,
        (s.elapsed_ms * 10.0).trunc() / 10.0
    )
}

/// One `[rules] closure` line per category set, in first-use order.
fn closure_lines(stats: &ClosureStats, registries: &Registries) -> Vec<String> {
    stats
        .sets
        .iter()
        .map(|s| {
            let (cats, mode) = (s.label(registries), s.modes());
            format!("[rules] closure cats={cats} mode={mode} bfs={} rows={}", s.bfs, s.rows)
        })
        .collect()
}

/// The user predicate behind `p` as `(name, arity)`: itself, or for a magic copy the
/// predicate it was made from (names split at the LAST `__`; an adorned copy keeps the
/// arity, a magic predicate's is its adornment's length). Explain elides the copies.
pub(crate) fn user_predicate(an: &Analysis, p: PredId) -> Option<(String, usize)> {
    let info = an.preds.get(p as usize)?;
    if !info.synthetic {
        return Some((info.name.clone(), info.arity));
    }
    let (rest, magic) = match info.name.strip_prefix("__a_") {
        Some(rest) => (rest, false),
        None => (info.name.strip_prefix("__m_")?, true),
    };
    let (name, adornment) = rest.rsplit_once("__")?;
    Some((name.to_string(), if magic { adornment.len() } else { info.arity }))
}

/// The outputs `wanted` names in `an` (module doc: empty is every sink), deduplicated.
fn resolve_outputs(an: &Analysis, wanted: &[String]) -> Result<Vec<PredId>, RulesError> {
    let user: Vec<PredId> = (0..an.preds.len())
        .filter(|&p| an.preds[p].kind == PredKind::Idb && !an.preds[p].synthetic)
        .map(|p| p as PredId)
        .collect();
    if wanted.is_empty() {
        let mut read = vec![false; an.preds.len()];
        for r in &an.rules {
            for lit in &r.body {
                if let CLit::Pos(p, ..) | CLit::Neg(p, ..) = lit
                    && *p != r.head
                    && let Some(slot) = read.get_mut(*p as usize)
                {
                    *slot = true;
                }
            }
        }
        return Ok(user.into_iter().filter(|&p| !read[p as usize]).collect());
    }
    let mut out = Vec::new();
    for w in wanted {
        let info = |p: &PredId| &an.preds[*p as usize];
        let Some(p) =
            user.iter().copied().find(|p| info(p).name == *w || info(p).signature() == *w)
        else {
            return Err(RulesError::Unknown { predicate: w.clone(), known: an.predicates() });
        };
        if !out.contains(&p) {
            out.push(p);
        }
    }
    Ok(out)
}

/// The magic rewrite for `outputs` and its re-analysis; the original program when
/// nothing was rewritten or the rewrite does not analyse (a guard: DA.12 drops every
/// step that would not).
fn demand(
    program: &Program,
    original: Analysis,
    catalog: &Catalog,
    outputs: &[PredId],
) -> (Program, Analysis, usize) {
    let Some((rewritten, report)) = magic::rewrite(program, &original, catalog, outputs) else {
        return (program.clone(), original, 0);
    };
    let mut names: Vec<&str> = report.rewritten.iter().map(|(p, _, _)| p.as_str()).collect();
    names.dedup();
    match analyse_with(&rewritten, catalog, true) {
        Ok(an) => (rewritten, an, names.len()),
        Err(_) => {
            for p in names {
                eprintln!("[rules] magic skipped pred={p} reason=unstratifiable");
            }
            (program.clone(), original, 0)
        }
    }
}

/// Does the program read an edge-family predicate or a closure builtin (so the edge
/// tiers are computed)?
fn reads_edges(an: &Analysis, catalog: &Catalog) -> bool {
    an.preds.iter().any(|p| match p.kind {
        PredKind::Edb(e) => matches!(e, EdbPred::Edge | EdbPred::EdgeConf | EdbPred::EdgeCell),
        PredKind::Builtin(b) => catalog
            .builtins
            .get(b)
            .is_some_and(|d| matches!(d.kind, BuiltinKind::Reaches | BuiltinKind::Scc)),
        PredKind::Idb => false,
    })
}

/// One family's restriction: the ids every atom names, or `None` once one names none.
fn narrow<I: Copy + PartialEq>(
    acc: &mut Option<Vec<I>>,
    t: Option<&CTerm>,
    id: impl Fn(&str) -> Option<I>,
) {
    let named = match t {
        Some(CTerm::Const(ConstVal::Str(name))) => id(name),
        _ => None,
    };
    match (acc.as_mut(), named) {
        (Some(ids), Some(i)) => {
            if !ids.contains(&i) {
                ids.push(i);
            }
        }
        _ => *acc = None,
    }
}

/// What the EDB loads keep: the categories every edge / edge_conf / edge_cell atom
/// names and the cell types every cell / edge_cell atom names (`None`: keep all).
fn restriction(an: &Analysis, catalog: &Catalog) -> Restrict {
    let reg = &catalog.registries;
    let category = |n: &str| reg.edge_categories.iter().find(|(_, x)| *x == n).map(|(id, _)| *id);
    let cell_type = |n: &str| reg.cell_types.iter().find(|(_, x)| *x == n).map(|(id, _)| *id);
    let mut cats: Option<Vec<EdgeCategoryId>> = Some(Vec::new());
    let mut cells: Option<Vec<CellTypeId>> = Some(Vec::new());
    for lit in an.rules.iter().flat_map(|r| &r.body) {
        let (CLit::Pos(p, terms, _) | CLit::Neg(p, terms, _)) = lit else { continue };
        let Some(PredKind::Edb(e)) = an.preds.get(*p as usize).map(|i| i.kind) else { continue };
        if matches!(e, EdbPred::Edge | EdbPred::EdgeConf | EdbPred::EdgeCell) {
            narrow(&mut cats, terms.get(2), category);
        }
        match e {
            EdbPred::Cell => narrow(&mut cells, terms.get(1), cell_type),
            EdbPred::EdgeCell => narrow(&mut cells, terms.get(3), cell_type),
            _ => {}
        }
    }
    if let Some(ids) = cats.as_mut() {
        ids.sort_by_key(|c| c.0);
    }
    if let Some(ids) = cells.as_mut() {
        ids.sort_by_key(|c| c.0);
    }
    Restrict { categories: cats, cell_types: cells }
}

/// A head part resolved before the body runs: a constant's handle or a variable.
#[derive(Clone, Copy)]
enum Out {
    Const(Handle),
    Var(u16),
}

/// Push `head`'s row for binding `b` onto `rows`; false (nothing pushed) when a head
/// variable is unbound, which safety (DA.5b) rules out.
fn push_head(head: &[Out], b: &Binding<'_>, rows: &mut Vec<Handle>) -> bool {
    let start = rows.len();
    for part in head {
        let h = match *part {
            Out::Const(h) => h,
            Out::Var(v) => b.vars.get(usize::from(v)).copied().unwrap_or(UNBOUND),
        };
        if h == UNBOUND {
            rows.truncate(start);
            return false;
        }
        rows.push(h);
    }
    true
}

/// The scc category sets `steps` call (seeds included), each once.
fn scc_sets(steps: &[Step], catalog: &Catalog, out: &mut Vec<CatSet>) {
    for s in steps {
        match s {
            Step::Call { builtin, cats: Some(c), .. }
            | Step::NegCall { builtin, cats: Some(c), .. }
                if catalog.builtins.get(*builtin).is_some_and(|d| d.kind == BuiltinKind::Scc)
                    && !out.contains(c) =>
            {
                out.push(c.clone());
            }
            // A seed nests one hoist per folded reaches: bounded by the rule's length.
            Step::ReachMulti { seed, .. } => scc_sets(seed, catalog, out),
            _ => {}
        }
    }
}

/// One round's candidate rows per own predicate (slot): flat rows and one meta each.
type Candidates = Vec<(Vec<Handle>, Vec<RowMeta>)>;

/// A recursive stratum's candidates held back for a later round of the level, by
/// height (module doc, "Minimal heights"). Ordered by height, never iterated as a hash.
#[derive(Default)]
struct Waiting {
    by_height: BTreeMap<u32, Candidates>,
}

impl Waiting {
    fn is_empty(&self) -> bool {
        self.by_height.is_empty()
    }

    /// The rows the next round inserts: of `adds` and the waiting rows, every one at
    /// most one above the lowest height among them; the rest wait. `None` when both are
    /// empty. `arity` is per slot.
    fn next(&mut self, mut adds: Candidates, arity: &[usize]) -> Option<Candidates> {
        let fresh_low = adds.iter().flat_map(|(_, metas)| metas.iter().map(|m| m.height)).min();
        let waiting_low = self.by_height.keys().next().copied();
        let low = match (fresh_low, waiting_low) {
            (Some(a), Some(b)) => a.min(b),
            (a, b) => a.or(b)?,
        };
        let cut = low.saturating_add(1);
        let slots = adds.len();
        for (slot, (rows, metas)) in adds.iter_mut().enumerate() {
            if metas.iter().all(|m| m.height <= cut) {
                continue;
            }
            let a = arity.get(slot).copied().unwrap_or(0);
            let (mut keep_rows, mut keep_metas) = (Vec::new(), Vec::new());
            for (i, meta) in metas.iter().enumerate() {
                let Some(row) = rows.get(i * a..(i + 1) * a) else { continue };
                if meta.height <= cut {
                    keep_rows.extend_from_slice(row);
                    keep_metas.push(*meta);
                } else {
                    let later = self
                        .by_height
                        .entry(meta.height)
                        .or_insert_with(|| vec![(Vec::new(), Vec::new()); slots]);
                    if let Some((r, m)) = later.get_mut(slot) {
                        r.extend_from_slice(row);
                        m.push(*meta);
                    }
                }
            }
            *rows = keep_rows;
            *metas = keep_metas;
        }
        while let Some(entry) = self.by_height.first_entry()
            && *entry.key() <= cut
        {
            for (slot, (rows, metas)) in entry.remove().into_iter().enumerate() {
                if let Some((r, m)) = adds.get_mut(slot) {
                    r.extend(rows);
                    m.extend(metas);
                }
            }
        }
        Some(adds)
    }
}

/// One evaluation's working state.
struct State<'t, S: FactSource + ?Sized> {
    src: &'t S,
    values: ValueTable,
    /// By `PredId`.
    rels: Vec<Relation>,
    /// Per `PredId`: rows per level, so a level's first round skips an input with none.
    at_level: Vec<[usize; 3]>,
    scc: Vec<(CatSet, Relation)>,
    closure: Closure<'t, S>,
    memo: Memo,
    budget: Budget,
    stats: EvalStats,
}

impl<'t, S: FactSource + ?Sized> State<'t, S> {
    /// exec's view of the state, for one run.
    fn ctx<'e>(
        &'e mut self,
        declared: &'e Declared<'t>,
        multi: &'e mut Vec<MultiState>,
    ) -> Ctx<'e, 't, S> {
        Ctx {
            src: self.src,
            values: &mut self.values,
            rels: &self.rels,
            scc: &self.scc,
            closure: &mut self.closure,
            builtins: &declared.impls,
            decls: &declared.catalog.builtins,
            memo: &mut self.memo,
            multi,
            budget: &mut self.budget,
        }
    }

    /// Load every EDB predicate the program references, once each, narrowed.
    fn load(&mut self, an: &Analysis, catalog: &Catalog, tiers: &[Tier]) {
        let restrict = restriction(an, catalog);
        for (p, info) in an.preds.iter().enumerate() {
            let PredKind::Edb(e) = info.kind else { continue };
            let rows = load_edb(self.src, e, &restrict, tiers, &mut self.values);
            for l in &rows.levels {
                self.at_level[p][usize::from(l.level())] += 1;
            }
            self.stats.edb_rows += self.rels[p].load(rows, None);
        }
    }

    /// Plan and run stratum `k`; its plan and ReachMulti states.
    fn stratum(
        &mut self,
        an: &Analysis,
        declared: &Declared<'t>,
        k: usize,
        user: &Program,
    ) -> Result<(StratumPlan, Vec<MultiState>), RulesError> {
        let catalog = &declared.catalog;
        let mut plan = {
            let rels = &self.rels;
            let size = |p: PredId| -> Option<usize> {
                let info = an.preds.get(p as usize)?;
                let complete = match info.kind {
                    PredKind::Edb(_) => true,
                    PredKind::Idb => info.stratum < k,
                    PredKind::Builtin(_) => false,
                };
                complete.then(|| rels.get(p as usize).map_or(0, Relation::len))
            };
            plan_stratum(an, catalog, k, &size)
        };
        let mut map = Vec::with_capacity(plan.indexes.len());
        for (p, perm) in &plan.indexes {
            match self.rels.get_mut(*p as usize).and_then(|r| r.ensure_index(perm)) {
                Some(i) => map.push(i),
                None => {
                    let sig = an.preds.get(*p as usize).map_or_else(String::new, |i| i.signature());
                    let message = format!(
                        "`{sig}` is too wide to index: a relation key holds at most 256 columns"
                    );
                    return Err(RulesError::Type { predicate: sig, message });
                }
            }
        }
        plan.resolve(&map);
        self.stats.variants += plan.rules.iter().map(|r| r.variants.len()).sum::<usize>();
        let mut sets = Vec::new();
        for v in plan.rules.iter().flat_map(|r| &r.variants) {
            scc_sets(&v.steps, catalog, &mut sets);
        }
        for set in sets {
            if !self.scc.iter().any(|(s, _)| *s == set) {
                self.materialise_scc(set);
            }
        }
        let mut multi = Vec::new();
        let stratum = &an.strata[k];
        if stratum.aggregate {
            self.aggregate_stratum(an, declared, &plan, &mut multi, user)?;
        } else {
            self.rule_stratum(an, declared, stratum, &plan, &mut multi, user)?;
        }
        for &p in &stratum.preds {
            if let Some(r) = self.rels.get_mut(p as usize) {
                r.seal();
            }
        }
        Ok((plan, multi))
    }

    /// scc's `(x, rep)` rows over `set`, with a `[1, 0]` index for a bound `rep`.
    fn materialise_scc(&mut self, set: CatSet) {
        let mut rows = EdbRows { arity: 2, ..EdbRows::default() };
        for (x, rep, t) in self.closure.scc_rows(&set) {
            rows.rows.push(self.values.intern(ValueRef::Node(x)));
            rows.rows.push(self.values.intern(ValueRef::Node(rep)));
            rows.levels.push(t);
        }
        let mut rel = Relation::new(2, &[vec![1, 0]]);
        rel.load(rows, None);
        self.scc.push((set, rel));
    }

    /// The head parts of a variant, constants interned.
    fn head(&mut self, parts: &[KeyPart]) -> Vec<Out> {
        let mut out = Vec::with_capacity(parts.len());
        for part in parts {
            let text;
            let v = match part {
                KeyPart::Var(v) => {
                    out.push(Out::Var(*v));
                    continue;
                }
                KeyPart::Const(ConstVal::Str(s)) => ValueRef::Str(s),
                KeyPart::Const(ConstVal::Int(i)) => ValueRef::Int(*i),
                // Heads take plain terms (DA.5b), so no head holds a set; it would read
                // as exec reads a category-set constant: names sorted, `|`-joined.
                KeyPart::Const(ConstVal::CatSet(set)) => {
                    let tables = self.src.tables();
                    let ids = match set {
                        CatSet::All => &[][..],
                        CatSet::Carry => tables.carry_edges,
                        CatSet::Of(ids) => ids.as_slice(),
                    };
                    let mut names: Vec<String> = ids
                        .iter()
                        .map(|c| {
                            let name = tables.registries.category_name(*c);
                            name.map_or_else(|| format!("#{}", c.0), str::to_string)
                        })
                        .collect();
                    names.sort();
                    text = if *set == CatSet::All { "*".to_string() } else { names.join("|") };
                    ValueRef::Str(&text)
                }
            };
            out.push(Out::Const(self.values.intern(v)));
        }
        out
    }

    /// Does variant `delta` of `rule` run in this round (module doc)?
    fn runs(
        &self,
        an: &Analysis,
        own: &[PredId],
        rule: &CRule,
        delta: Option<usize>,
        level: Tier,
        first: bool,
    ) -> bool {
        if first && level == Tier::Fact {
            return delta.is_none();
        }
        let Some(CLit::Pos(p, ..)) = delta.and_then(|d| rule.body.get(d)) else { return false };
        let mine = own.contains(p);
        if !first {
            return mine && self.rels.get(*p as usize).is_some_and(|r| r.recent_len() > 0);
        }
        match an.preds.get(*p as usize).map(|i| i.kind) {
            Some(PredKind::Builtin(_)) => true,
            Some(PredKind::Idb | PredKind::Edb(_)) => {
                !mine
                    && self
                        .at_level
                        .get(*p as usize)
                        .is_some_and(|n| n[usize::from(level.level())] > 0)
            }
            None => false,
        }
    }

    /// A rule stratum's three levelled fixpoints.
    fn rule_stratum(
        &mut self,
        an: &Analysis,
        declared: &Declared<'t>,
        stratum: &Stratum,
        plan: &StratumPlan,
        multi: &mut Vec<MultiState>,
        user: &Program,
    ) -> Result<(), RulesError> {
        let own = stratum.preds.as_slice();
        let arity: Vec<usize> =
            own.iter().map(|&p| an.preds.get(p as usize).map_or(0, |i| i.arity)).collect();
        for level in Tier::ALL {
            let mut first = true;
            let mut waiting = Waiting::default();
            loop {
                let mut adds: Vec<(Vec<Handle>, Vec<RowMeta>)> =
                    vec![(Vec::new(), Vec::new()); own.len()];
                let mut ran = false;
                for rp in &plan.rules {
                    let Some(rule) = an.rules.get(rp.rule) else { continue };
                    let Some(slot) = own.iter().position(|p| *p == rule.head) else { continue };
                    // evaluate checked that every rule index fits below EDB_RULE.
                    let meta_rule = u16::try_from(rp.rule).unwrap_or(EDB_RULE);
                    for v in &rp.variants {
                        if !self.runs(an, own, rule, v.delta, level, first) {
                            continue;
                        }
                        ran = true;
                        let head = self.head(&v.head);
                        let (rows, metas) = &mut adds[slot];
                        let mode = Mode::Eval { level, first_round: first };
                        run_body(&mut self.ctx(declared, multi), &v.steps, mode, &mut |b| {
                            if push_head(&head, b, rows) {
                                metas.push(RowMeta {
                                    height: b.height.saturating_add(1),
                                    rule: meta_rule,
                                });
                            }
                            Ok(())
                        })?;
                        if self.budget.tripped.is_some() {
                            return Err(self.budget_error(an, rule.head, user));
                        }
                    }
                }
                let batch = if stratum.recursive {
                    waiting.next(adds, &arity)
                } else {
                    ran.then_some(adds)
                };
                let Some(mut batch) = batch else {
                    if ran {
                        self.stats.rounds += 1;
                    }
                    break;
                };
                let mut fresh = 0;
                for (slot, &p) in own.iter().enumerate() {
                    let (rows, metas) = &mut batch[slot];
                    let n = self.rels[p as usize].insert_round(level, rows, metas);
                    self.at_level[p as usize][usize::from(level.level())] += n;
                    self.stats.derived_rows += n;
                    fresh += n;
                    if self.budget.charge(n) {
                        return Err(self.budget_error(an, p, user));
                    }
                }
                self.stats.rounds += 1;
                first = false;
                if !stratum.recursive || (fresh == 0 && waiting.is_empty()) {
                    break;
                }
            }
        }
        Ok(())
    }

    /// An aggregate stratum: its one body over every level, then the fold.
    fn aggregate_stratum(
        &mut self,
        an: &Analysis,
        declared: &Declared<'t>,
        plan: &StratumPlan,
        multi: &mut Vec<MultiState>,
        user: &Program,
    ) -> Result<(), RulesError> {
        let (Some(rp), Some(spec)) = (plan.rules.first(), plan.agg.as_ref()) else { return Ok(()) };
        let (Some(rule), Some(v)) = (an.rules.get(rp.rule), rp.variants.first()) else {
            return Ok(());
        };
        let p = rule.head;
        let head = self.head(&v.head);
        let (mut bindings, mut levels, mut heights) = (Vec::new(), Vec::new(), Vec::new());
        let mode = Mode::Eval { level: Tier::Heuristic, first_round: true };
        run_body(&mut self.ctx(declared, multi), &v.steps, mode, &mut |b| {
            if push_head(&head, b, &mut bindings) {
                levels.push(b.level);
                heights.push(b.height);
            }
            Ok(())
        })?;
        if self.budget.tripped.is_some() {
            return Err(self.budget_error(an, p, user));
        }
        let sig = an.preds[p as usize].signature();
        let src = self.src;
        let render = |id: NodeId| src.render_node(id);
        let mut cmp = |t: &mut ValueTable, a: Handle, b: Handle| t.cmp_rendered(a, b, &render);
        let out = aggregate::aggregate(
            bindings,
            head.len(),
            levels,
            heights,
            spec,
            &mut self.values,
            &mut cmp,
            &sig,
        )?;
        self.stats.aggregates += 1;
        for l in &out.levels {
            self.at_level[p as usize][usize::from(l.level())] += 1;
        }
        let arity = an.preds[p as usize].arity;
        let rows = EdbRows { arity, rows: out.rows, levels: out.levels, ordinals: Vec::new() };
        let n = self.rels[p as usize].load(rows, Some(&out.heights));
        self.stats.derived_rows += n;
        if self.budget.charge(n) {
            return Err(self.budget_error(an, p, user));
        }
        Ok(())
    }

    /// The budget error for predicate `p` (a magic copy names its user predicate, and
    /// the line is that predicate's first rule in the user's program), printing the
    /// `[rules] budget` marker.
    fn budget_error(&self, an: &Analysis, p: PredId, user: &Program) -> RulesError {
        let (name, arity) = user_predicate(an, p).unwrap_or_default();
        let predicate = format!("{name}/{arity}");
        let first = user.rules.iter().find(|r| r.head.pred == name && r.head.arity() == arity);
        let (source, line) = match first {
            Some(r) => (user.source_name(r.span.source).to_string(), r.span.line),
            None => (user.source_name(0).to_string(), 0),
        };
        let (rows, cap) = (self.budget.tripped.unwrap_or(self.budget.used), self.budget.cap);
        eprintln!("[rules] budget pred={predicate} rows={rows} cap={cap}");
        RulesError::Budget { predicate, source, line, rows, cap }
    }
}

/// Per rule: its rows' absence caveats - the rule's own negations plus the negation
/// cones of its positive IDB atoms, magic predicates skipped (their negations are the
/// site prefix's, which the site's own rule already holds) - deduplicated and sorted.
fn rule_caveats(an: &Analysis) -> Vec<Vec<AbsenceCaveat>> {
    let n = an.preds.len();
    let mut rules_of: Vec<Vec<usize>> = vec![Vec::new(); n];
    for r in &an.rules {
        if let Some(v) = rules_of.get_mut(r.head as usize) {
            v.push(r.index);
        }
    }
    let followed = |p: PredId| {
        an.preds
            .get(p as usize)
            .is_some_and(|i| i.kind == PredKind::Idb && !i.name.starts_with("__m_"))
    };
    let reads = |r: &CRule| -> Vec<PredId> {
        r.body
            .iter()
            .filter_map(|l| match l {
                CLit::Pos(p, ..) if followed(*p) => Some(*p),
                _ => None,
            })
            .collect()
    };
    // cone[p]: the negations of every rule reachable from p through positive atoms.
    let cones: Vec<Vec<&NegRead>> = (0..n as PredId)
        .map(|start| {
            let mut out = Vec::new();
            if !followed(start) {
                return out;
            }
            let (mut seen, mut stack) = (vec![false; n], vec![start]);
            seen[start as usize] = true;
            while let Some(q) = stack.pop() {
                for &r in &rules_of[q as usize] {
                    let Some(rule) = an.rules.get(r) else { continue };
                    out.extend(&rule.negated_reads);
                    for next in reads(rule) {
                        if !seen[next as usize] {
                            seen[next as usize] = true;
                            stack.push(next);
                        }
                    }
                }
            }
            out
        })
        .collect();
    an.rules
        .iter()
        .map(|r| {
            let mut all: Vec<AbsenceCaveat> = r
                .negated_reads
                .iter()
                .chain(reads(r).into_iter().flat_map(|q| cones[q as usize].iter().copied()))
                .map(|n| AbsenceCaveat {
                    negated: n.negated.clone(),
                    categories: n.categories.clone(),
                    any_category: n.any_category,
                })
                .collect();
            all.sort_by(|a, b| {
                (&a.negated, &a.categories, a.any_category).cmp(&(
                    &b.negated,
                    &b.categories,
                    b.any_category,
                ))
            });
            all.dedup();
            all
        })
        .collect()
}

/// The rendered keys of the node handles a row set holds, for `rows()`: the order of
/// `ValueTable::cmp_rendered` (type rank Int < Str < Node, then the value, a node by
/// its render key, then its id), computed per call because that table's render cache
/// is written only through `&mut`.
struct Keys<'v> {
    values: &'v ValueTable,
    /// Sorted by handle.
    nodes: Vec<(Handle, String)>,
}

impl<'v> Keys<'v> {
    fn new<S: FactSource + ?Sized>(
        src: &S,
        values: &'v ValueTable,
        handles: impl Iterator<Item = Handle>,
    ) -> Self {
        let mut hs: Vec<Handle> =
            handles.filter(|h| matches!(values.get(*h), ValueRef::Node(_))).collect();
        hs.sort_unstable();
        hs.dedup();
        let nodes = hs
            .into_iter()
            .map(|h| match values.get(h) {
                ValueRef::Node(id) => (h, src.render_node(id)),
                ValueRef::Str(_) | ValueRef::Int(_) => (h, String::new()),
            })
            .collect();
        Keys { values, nodes }
    }

    fn key(&self, h: Handle) -> &str {
        self.nodes.binary_search_by_key(&h, |(k, _)| *k).map_or("", |i| self.nodes[i].1.as_str())
    }

    fn cmp(&self, a: Handle, b: Handle) -> Ordering {
        if a == b {
            return Ordering::Equal;
        }
        let rank = |v: ValueRef<'_>| match v {
            ValueRef::Int(_) => 0u8,
            ValueRef::Str(_) => 1,
            ValueRef::Node(_) => 2,
        };
        match (self.values.get(a), self.values.get(b)) {
            (ValueRef::Int(x), ValueRef::Int(y)) => x.cmp(&y),
            (ValueRef::Str(x), ValueRef::Str(y)) => x.cmp(y),
            (ValueRef::Node(x), ValueRef::Node(y)) => {
                self.key(a).cmp(self.key(b)).then(x.0.cmp(&y.0))
            }
            (va, vb) => rank(va).cmp(&rank(vb)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell as Counter, RefCell};

    use glia_activation::algo::GraphSource;
    use glia_activation::profile::DomainTables;
    use glia_core::{Cell, Confidence, Edge, NodeKindId};

    use super::*;
    use crate::ir::AttrDecl;
    use crate::source::MemSource;
    use crate::testkit::{GraphSpec, category_id, chain, clique_ring, lcg_graph, node_id, tables};

    const FUNCTION: NodeKindId = NodeKindId(1);
    const ROUTE: NodeKindId = NodeKindId(3);

    /// Spec test 1's program: recursion, a negation, an aggregate and a hoisted reach.
    const MIXED: &str = r#"tc(x, y) :- edge(x, y, "CALLS").
tc(x, z) :- tc(x, y), edge(y, z, "CALLS").
handled(r) :- edge(r, _, "HANDLED_BY").
unhandled(r) :- node(r, "ROUTE"), !handled(r).
fan(y, count(x)) :- edge(x, y, "CALLS").
far(b) :- node(a, "ROUTE"), reaches(a, b, "CALLS").
"#;

    const TC: &str = r#"tc(x, y) :- edge(x, y, "CALLS").
tc(x, z) :- tc(x, y), edge(y, z, "CALLS").
"#;

    /// Two routes r1 / r2 and four functions f1..f4: r1 is HANDLED_BY f1; CALLS r1 -> f1
    /// (DERIVED), f1 -> f2, f1 -> f4, f4 -> f3, r2 -> f2 (FACT) and f2 -> f3 (HEURISTIC).
    fn mixed() -> MemSource {
        let mut g = MemSource::new(tables());
        let names = ["r1", "r2", "f1", "f2", "f3", "f4"];
        for (i, name) in names.iter().enumerate() {
            let kind = if name.starts_with('r') { ROUTE } else { FUNCTION };
            g.node(node_id(0, i as u32), kind, *name);
        }
        let at = |n: &str| node_id(0, names.iter().position(|x| *x == n).unwrap_or(0) as u32);
        for (from, to, cat, tier) in [
            ("r1", "f1", "HANDLED_BY", Tier::Fact),
            ("r1", "f1", "CALLS", Tier::Derived),
            ("f1", "f2", "CALLS", Tier::Fact),
            ("f2", "f3", "CALLS", Tier::Heuristic),
            ("f1", "f4", "CALLS", Tier::Fact),
            ("f4", "f3", "CALLS", Tier::Fact),
            ("r2", "f2", "CALLS", Tier::Fact),
        ] {
            g.edge(at(from), at(to), category_id(cat), Confidence::Strong, tier);
        }
        g
    }

    /// `n` numbered testkit nodes (`n<i:05>`, `index` = i) and no edges.
    fn nodes(n: u32) -> MemSource {
        let spec = GraphSpec {
            nodes: n,
            edges: 0,
            categories: vec![],
            tier_mix: [100, 0, 0],
            seed: 1,
            salt: 0,
        };
        lcg_graph(&spec)
    }

    fn edge(g: &mut MemSource, from: u32, to: u32, cat: &str, tier: Tier) {
        g.edge(node_id(0, from), node_id(0, to), category_id(cat), Confidence::Strong, tier);
    }

    fn eval<'s, S: FactSource + ?Sized>(text: &str, g: &'s S, opts: &Options) -> Evaluation<'s, S> {
        run("rules", text, g, opts).unwrap_or_else(|e| panic!("evaluation failed: {e}"))
    }

    /// Each row of `pred` as its rendered values joined by spaces, with its tier.
    fn rendered<S: FactSource + ?Sized>(ev: &Evaluation<'_, S>, pred: &str) -> Vec<(String, Tier)> {
        let rows = ev.rows(pred).unwrap_or_else(|e| panic!("rows({pred}): {e}"));
        rows.iter()
            .map(|r| (r.values.iter().map(|v| ev.render(v)).collect::<Vec<_>>().join(" "), r.tier))
            .collect()
    }

    fn rows(want: &[(&str, Tier)]) -> Vec<(String, Tier)> {
        want.iter().map(|(s, t)| (s.to_string(), *t)).collect()
    }

    #[test]
    fn small_program_exact_rows() {
        let g = mixed();
        let ev = eval(MIXED, &g, &Options::default());
        use Tier::{Derived as D, Fact as F, Heuristic as H};
        // f2 -> f3 is the one HEURISTIC edge: a pair crossing it is HEURISTIC unless
        // another path is stronger (f1 -> f4 -> f3 is FACT, r1's paths start DERIVED).
        assert_eq!(
            rendered(&ev, "tc"),
            rows(&[
                ("f1 f2", F),
                ("f1 f3", F),
                ("f1 f4", F),
                ("f2 f3", H),
                ("f4 f3", F),
                ("r1 f1", D),
                ("r1 f2", D),
                ("r1 f3", D),
                ("r1 f4", D),
                ("r2 f2", F),
                ("r2 f3", H),
            ])
        );
        assert_eq!(rendered(&ev, "unhandled/1"), rows(&[("r2", F)]));
        let unhandled = ev.rows("unhandled").unwrap_or_default();
        let caveat = AbsenceCaveat {
            negated: "handled/1".into(),
            categories: vec!["HANDLED_BY".into()],
            any_category: false,
        };
        assert_eq!(unhandled[0].absence, vec![caveat]);
        // A group takes its weakest contributing row: f3's callers are f2 (via the
        // HEURISTIC edge) and f4.
        assert_eq!(
            rendered(&ev, "fan"),
            rows(&[("f1 1", D), ("f2 2", F), ("f3 2", H), ("f4 1", F)])
        );
        assert_eq!(
            ev.rows("fan").unwrap_or_default()[1].values,
            vec![Value::Node(node_id(0, 3)), Value::Int(2)]
        );
        // One multi-source walk from r1 and r2: the best tier over both sources.
        assert_eq!(rendered(&ev, "far"), rows(&[("f1", D), ("f2", F), ("f3", D), ("f4", D)]));
        for p in ["tc", "fan", "far"] {
            assert!(ev.rows(p).unwrap_or_default().iter().all(|r| r.absence.is_empty()), "{p}");
        }
        // The warning: tc's base rule starts from every CALLS edge.
        let warnings: Vec<String> = ev.warnings().iter().map(|d| d.message.clone()).collect();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("recursive predicate `tc/2` is unseeded"), "{warnings:?}");
    }

    #[test]
    fn best_derivation_wins() {
        let mut g = nodes(3);
        edge(&mut g, 0, 2, "CALLS", Tier::Heuristic);
        edge(&mut g, 0, 1, "CALLS", Tier::Fact);
        edge(&mut g, 1, 2, "CALLS", Tier::Fact);
        let ev = eval(TC, &g, &Options::default());
        assert_eq!(
            rendered(&ev, "tc"),
            rows(&[
                ("n00000 n00001", Tier::Fact),
                ("n00000 n00002", Tier::Fact),
                ("n00001 n00002", Tier::Fact)
            ])
        );
        // The FACT 2-hop path is found in FACT's second round, so its height is 2; the
        // HEURISTIC hop never inserts a second copy.
        let tc = ev.rows("tc").unwrap_or_default();
        assert_eq!(tc[1].at.meta.height, 2);
        assert_eq!(ev.stats().derived_rows, 3);
    }

    #[test]
    fn budget_stops_with_the_text() {
        let g = clique_ring(60, 120, "CALLS", 1);
        let opts = Options { max_rows: 1000, ..Options::default() };
        let err = match run("rules", TC, &g, &opts) {
            Ok(_) => panic!("an unseeded closure of 3,600 rows passed a budget of 1,000"),
            Err(e) => e,
        };
        let RulesError::Budget { predicate, source, line, rows, cap } = &err else {
            panic!("expected a budget error, got {err:?}")
        };
        assert_eq!((predicate.as_str(), source.as_str(), *line, *cap), ("tc/2", "rules", 1, 1000));
        assert!(*rows > 1000, "{rows}");
        assert!(
            err.to_string().starts_with("rule evaluation stopped: `tc/2` (rules:1) reached "),
            "{err}"
        );
        // The default budget is the spec's and the same closure runs under it.
        assert_eq!(Options::default().max_rows, DEFAULT_MAX_ROWS);
        assert_eq!(DEFAULT_MAX_ROWS, 5_000_000);
        assert_eq!(eval(TC, &g, &Options::default()).rows("tc").map(|r| r.len()).ok(), Some(3600));
    }

    #[test]
    fn outputs_default_and_unknown() {
        let g = mixed();
        let ev = eval(MIXED, &g, &Options::default());
        let outputs = ["fan/2", "far/1", "tc/2", "unhandled/1"];
        assert_eq!(ev.outputs(), outputs);
        let err = ev.rows("nope").map(|r| r.len());
        assert_eq!(
            err,
            Err(RulesError::Unknown {
                predicate: "nope".into(),
                known: outputs.map(String::from).to_vec()
            })
        );
        // handled/1 is read by unhandled, so it is no output and rows() does not answer it.
        assert!(matches!(ev.rows("handled"), Err(RulesError::Unknown { .. })));
        // Named outputs: by name or signature, deduplicated, sorted.
        let opts = Options {
            outputs: vec!["unhandled".into(), "handled/1".into(), "handled".into()],
            ..Options::default()
        };
        let named = eval(MIXED, &g, &opts);
        assert_eq!(named.outputs(), ["handled/1", "unhandled/1"]);
        assert_eq!(rendered(&named, "handled"), rows(&[("r1", Tier::Fact)]));
        // A name the program does not define lists what it does, in source order.
        let opts = Options { outputs: vec!["tc/3".into()], ..Options::default() };
        let known =
            ["tc/2", "handled/1", "unhandled/1", "fan/2", "far/1"].map(String::from).to_vec();
        assert_eq!(
            run("rules", MIXED, &g, &opts).map(|e| e.outputs()),
            Err(RulesError::Unknown { predicate: "tc/3".into(), known })
        );
        // run reports a parse error as the parser gives it.
        assert!(matches!(
            run("rules", "p(x :- q.", &g, &Options::default()),
            Err(RulesError::Parse(_))
        ));
    }

    /// A source counting attribute reads and cell walks, over a MemSource.
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
    fn edb_is_lazy() {
        // Three CALLS hops and two USES edges.
        let mut inner = chain(4, "CALLS", &[Tier::Fact]);
        edge(&mut inner, 0, 2, "USES", Tier::Fact);
        edge(&mut inner, 1, 3, "USES", Tier::Derived);
        let g =
            Counting { inner, cell_walks: Counter::new(0), attr_reads: RefCell::new(Vec::new()) };
        let calls = eval(r#"p(x, y) :- edge(x, y, "CALLS")."#, &g, &Options::default());
        assert_eq!(calls.stats().edb_rows, 3, "every edge atom says CALLS: only CALLS loads");
        assert_eq!(calls.rows("p").map(|r| r.len()).ok(), Some(3));
        assert!(g.attr_reads.borrow().is_empty());
        assert_eq!(g.cell_walks.get(), 0);
        // Two families share one category restriction; a variable category keeps all.
        let two = eval(
            r#"p(x, y) :- edge(x, y, "CALLS"), !edge_conf(y, x, "USES", "weak")."#,
            &g,
            &Options::default(),
        );
        assert_eq!(two.stats().edb_rows, 5 + 5, "edge and edge_conf each keep CALLS | USES");
        let any = eval("p(x, y) :- edge(x, y, c), category(c).", &g, &Options::default());
        assert_eq!(any.stats().edb_rows, 5 + 6, "five edges, six registered categories");
        assert!(g.attr_reads.borrow().is_empty());
        assert_eq!(g.cell_walks.get(), 0);
        // An attribute is read when, and only when, a rule names it.
        let idx = eval("p(x) :- index(x, 2).", &g, &Options::default());
        assert_eq!(rendered(&idx, "p"), rows(&[("n00002", Tier::Fact)]));
        assert_eq!(*g.attr_reads.borrow(), [0]);
        assert_eq!(g.cell_walks.get(), 0);
    }

    #[test]
    fn marker_line() {
        let g = mixed();
        let ev = eval(MIXED, &g, &Options::default());
        let mut s = ev.stats().clone();
        assert!(s.elapsed_ms >= 0.0);
        s.elapsed_ms = 0.0;
        assert_eq!(
            eval_line(&s, ev.warnings().len()),
            "[rules] eval strata=5 rules=6 variants=12 edb=13 derived=21 rounds=16 aggregates=1 \
             magic=0 values=12 warnings=1 ms=0.0"
        );
        s.elapsed_ms = 12.36;
        assert!(eval_line(&s, 0).ends_with(" warnings=0 ms=12.3"), "{}", eval_line(&s, 0));
        let registries = tables().registries;
        assert_eq!(
            closure_lines(&ev.closure_stats, &registries),
            ["[rules] closure cats=CALLS mode=multi bfs=1 rows=4"]
        );
    }

    /// Spec test 7's program over a chain: n00000 seeds q, n00001 seeds r.
    const FALLBACK: &str = r#"seed(x) :- index(x, 0).
seed2(x) :- index(x, 1).
tc(x, y) :- edge(x, y, "CALLS").
tc(x, z) :- tc(x, y), edge(y, z, "CALLS").
q(y) :- seed(x), !r(x), tc(x, y).
r(x) :- seed2(x), tc(x, _z).
"#;

    #[test]
    fn magic_fallback() {
        let g = chain(5, "CALLS", &[Tier::Fact]);
        let on = Options { outputs: vec!["q".into()], ..Options::default() };
        let off = Options { magic: false, ..on.clone() };
        let (a, b) = (eval(FALLBACK, &g, &on), eval(FALLBACK, &g, &off));
        // The rewrite would put `!r` inside recursion, so the original program runs.
        assert_eq!(a.stats().magic_rewrites, 0);
        assert_eq!(a.program, b.program);
        let want = rows(&[
            ("n00001", Tier::Fact),
            ("n00002", Tier::Fact),
            ("n00003", Tier::Fact),
            ("n00004", Tier::Fact),
        ]);
        assert_eq!(rendered(&a, "q"), want);
        assert_eq!(rendered(&b, "q"), want);
        assert_eq!(a.rows("q"), b.rows("q"));
        // q's rows depend on r's absence; r reads CALLS through tc.
        let caveat = AbsenceCaveat {
            negated: "r/1".into(),
            categories: vec!["CALLS".into()],
            any_category: false,
        };
        assert!(a.rows("q").unwrap_or_default().iter().all(|r| r.absence == [caveat.clone()]));
    }

    #[test]
    fn magic_seeds_user_recursion() {
        let g = clique_ring(30, 40, "CALLS", 3);
        let text = format!("seed(x) :- index(x, 0).\n{TC}out(b) :- seed(a), tc(a, b).\n");
        let on = eval(&text, &g, &Options::default());
        let off = eval(&text, &g, &Options { magic: false, ..Options::default() });
        assert_eq!(on.outputs(), ["out/1"]);
        assert_eq!(on.stats().magic_rewrites, 1);
        assert_eq!(off.stats().magic_rewrites, 0);
        // The ring is strongly connected: n00000 reaches every node, itself included.
        assert_eq!(rendered(&on, "out").len(), 30);
        assert_eq!(rendered(&on, "out"), rendered(&off, "out"));
        // Off derives all 900 pairs; on derives n00000's 30 and its demand.
        assert!(off.stats().derived_rows > 900, "{:?}", off.stats());
        assert!(on.stats().derived_rows < 100, "{:?}", on.stats());
        // tc is no output, and with magic on only its demanded copy exists.
        assert!(matches!(on.rows("tc"), Err(RulesError::Unknown { .. })));
        assert_eq!(on.warnings(), off.warnings(), "warnings are the original program's");
        // A budget stop inside the copy names the user predicate and its first rule.
        let tight = Options { max_rows: 40, ..Options::default() };
        let err = run("rules", &text, &g, &tight).map(|e| e.outputs());
        let named = match &err {
            Err(RulesError::Budget { predicate, line, .. }) => Some((predicate.as_str(), *line)),
            _ => None,
        };
        assert_eq!(named, Some(("tc/2", 2)), "{err:?}");
    }

    #[test]
    fn levelled_first_round() {
        // a -> b -> c FACT, c -> d DERIVED.
        let g = chain(4, "CALLS", &[Tier::Fact, Tier::Fact, Tier::Derived]);
        let ev = eval(TC, &g, &Options::default());
        use Tier::{Derived as D, Fact as F};
        assert_eq!(
            rendered(&ev, "tc"),
            rows(&[
                ("n00000 n00001", F),
                ("n00000 n00002", F),
                ("n00000 n00003", D),
                ("n00001 n00002", F),
                ("n00001 n00003", D),
                ("n00002 n00003", D),
            ])
        );
        // tc(a, d) comes from level DERIVED's FIRST round: the recursive rule's edge
        // delta (the level-DERIVED partition) joined with the FACT row tc(a, c), height
        // 2; later rounds read only tc's recent rows, which never meet c -> d.
        let tc = ev.rows("tc").unwrap_or_default();
        assert_eq!(tc[2].at.meta, RowMeta { height: 3, rule: 1 });
        assert_eq!(tc[2].at.level, D);
        // FACT: 3 rounds (2 hops, then a -> c, then nothing); DERIVED: 3 (c -> d at
        // height 1 and b -> d at 2, a -> d at 3 held back; then a -> d; then nothing);
        // HEURISTIC: no level-HEURISTIC input, so none.
        assert_eq!(ev.stats().rounds, 6);
    }

    #[test]
    fn scc_and_reaches_rows_carry_their_tiers() {
        // 0 -> 1 -> 2 -> 0 with 2 -> 0 DERIVED; 3 -> 0 FACT.
        let mut h = nodes(4);
        for (f, t, tier) in
            [(0, 1, Tier::Fact), (1, 2, Tier::Fact), (2, 0, Tier::Derived), (3, 0, Tier::Fact)]
        {
            edge(&mut h, f, t, "CALLS", tier);
        }
        let text = r#"cyc(x, r) :- scc(x, r, "CALLS").
hit(x, y) :- node(x, _), reaches(x, y, "CALLS").
"#;
        let ev = eval(text, &h, &Options::default());
        use Tier::{Derived as D, Fact as F};
        assert_eq!(
            rendered(&ev, "cyc"),
            rows(&[("n00000 n00000", D), ("n00001 n00000", D), ("n00002 n00000", D)])
        );
        let hit = rendered(&ev, "hit");
        assert!(hit.contains(&("n00003 n00001".into(), F)), "{hit:?}");
        assert!(!hit.iter().any(|(r, _)| r == "n00003 n00003"), "n00003 is on no cycle: {hit:?}");
        assert!(hit.contains(&("n00000 n00000".into(), D)), "{hit:?}");
    }
}
