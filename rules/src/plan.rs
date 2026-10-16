//! Per-stratum rule plans: semi-naive variants, bound-first order, index choice.
//!
//! What evaluation (DA.14b), aggregation (DA.8) and explain (DA.15) hand the step
//! interpreter (DA.14a). A [`StratumPlan`] holds one [`RulePlan`] per rule of the
//! stratum, in program order, every relation index those plans read, and the number
//! of hoisted-reach slots they use.
//!
//! **Variants.** A rule plan holds one [`Variant`] per delta-capable body literal `i`
//! (a positive atom of a relation - this stratum's, a lower stratum's or the EDB's -
//! or a `tiered` builtin), plus `delta: None`. In variant `i` the literals `j < i`
//! read [`Src::Old`], `i` reads [`Src::Delta`] and `j > i` read [`Src::All`]: the
//! standard semi-naive assignment, by literal INDEX and independent of the order the
//! steps run in, so the variants of a round split its new derivations by the first
//! literal (in source order) whose row is new. The tier lattice is evaluated level by
//! level (DA.14b), which is why the EDB and lower strata get variants too: in the
//! first round of level L their level-L partition is the delta that can create new
//! derivations. `delta: None` reads everything `All`; it runs only in the first round
//! of the first level. DA.14b states which rows each `Src` reads at each level and
//! round; `Variant::delta` is listed first-None, then by literal index. An untiered
//! builtin takes its literal's `Src` like any other step but is never a delta: its
//! rows are all FACT, so past the first level `Old` and `All` read the same rows.
//!
//! **Order.** Each variant's steps follow `analysis::sips_order` (DA.5b), the one
//! bound-first order the magic rewrite also reads, with the delta literal forced first
//! when it is a relation atom (the delta is the smallest input). The caller's `size`
//! breaks ties; a stratum is planned just before it runs, so the EDB and lower strata
//! are complete and their sizes are real, while the stratum's own predicates are
//! still growing and read as unknown here whatever `size` says. Ties then go to
//! source order.
//!
//! **Relation steps and indexes.** A positive relation atom with no bound column is a
//! [`Step::Scan`] of the identity permutation; otherwise a [`Step::Lookup`] whose
//! permutation lists the bound columns (constants and bound variables) first, in
//! column order, then the rest, so its key is a prefix of that index (DA.7). A
//! negated atom is a [`Step::Neg`] keyed the same way: its wildcards are the trailing
//! columns of the permutation it requests. Every relation step names its
//! permutation (`perm`) and its position in the plan's index list (`index`).
//! [`StratumPlan::indexes`] is sorted by predicate then permutation and deduplicated,
//! and holds the identity of every predicate the stratum reads or defines (it sorts
//! first among a predicate's permutations); DA.14b builds the stratum's relations with
//! `Relation::new` and the indexes lower relations lack with `Relation::ensure_index`,
//! then [`StratumPlan::resolve`] rewrites every `index` to the relation's own index
//! number. `binds` and `checks` name the atom's own (unpermuted) columns: a bind sets
//! a variable from that column of the row; a check, applied after the step's binds,
//! requires the column to equal its part (a variable repeated inside one atom).
//!
//! **Builtins and comparisons.** A positive builtin is a [`Step::Call`] (a domain or
//! string builtin, or `scc`, which reads the relation DA.14b materialises per category
//! set from `Closure::scc_rows`; `cats` names that set) or a [`Step::Reach`] (`from` /
//! `to` are the bound ends, `None` a free one; both free is every source, budgeted).
//! A negated builtin is a [`Step::NegCall`] / [`Step::NegReach`], which holds when the
//! call yields no row (a `None` argument is a wildcard). A comparison of two bound
//! sides is a [`Step::Cmp`]; an `=` with one free variable side is an
//! [`Step::Assign`]; a comparison with `_` on a side constrains nothing (`_` stands
//! for some value) and plans no step.
//!
//! **The reaches hoist (R1 sections 1.4 and 3).** At a positive `reaches` whose source
//! end is a bound variable (X; or Y when only Y is bound: a backward walk) and whose
//! other end is a free variable, the base order (the `delta: None` one) hoists the
//! call when (a) no variable bound before it is live - read by a later literal,
//! comparisons and negations included, or by the head, aggregated variables included -
//! so X itself is dead after the call, and (b) every literal before it reads complete
//! input (the EDB, a lower stratum, a builtin, a negation; never a positive atom of
//! the stratum being planned). The steps so far become [`Step::ReachMulti`]'s `seed`,
//! which exec runs to completion once to collect the distinct seeds with their best
//! level, followed by ONE levelled multi-source BFS that binds `bind`: one walk for
//! every source instead of one per source, the unary seeded shape R1 measured several
//! times cheaper than the pairwise one. Hoists nest: a later qualifying `reaches` folds
//! the earlier ReachMulti into its own seed. The outermost one is the rule's composite:
//! its members are every literal before it in the base order plus itself; it runs
//! first in every variant (its seeds are complete input, so it is the same in each,
//! and all of them share its `slot`: exec keeps one seed set per slot); it counts as
//! ONE delta-capable tiered literal at its `reaches` literal's index (its rows of level
//! L are new input in level L's first round), and its members get no variants of their
//! own. Steps inside a seed read `Src::All`. A call that fails the condition is a
//! per-binding Reach (memoised per source by DA.11), as it is after any recursive
//! prefix.
//!
//! **Aggregates and explain.** An aggregate rule has one body plan, every step
//! `Src::All` (DA.14b runs it once over the complete lower strata), whose bindings
//! are projected to the group parts (the head's non-aggregate terms, in head order,
//! constants included) and then the distinct aggregated variables, with the
//! [`AggSpec`] DA.8 evaluates over that projection ([`plan_aggregate`]; a
//! [`plan_stratum`] of an aggregate stratum carries the same). [`plan_bound`] is the
//! explain plan: the head's variables pre-bound, every `Src::All`, never a hoist
//! (explain needs every literal's instantiation), and exactly the index permutations
//! its steps read, which explain obtains through `Relation::ensure_index` because the
//! evaluation plan may never have built them.
//!
//! Deterministic: no hash map, every choice ties to literal index, and a plan is a
//! function of the analysis, the catalog and the sizes.

use crate::aggregate::{AggSpec, OutCol};
use crate::analysis::{
    Analysis, CLit, CRule, CTerm, CatSet, ConstVal, PredId, PredKind, closure_set, sips_order,
};
use crate::ir::{BuiltinKind, Catalog, CmpOp};

/// Which rows a step reads at the level and round being run (DA.14b maps each to a
/// relation view).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Src {
    /// Rows from before this round.
    Old,
    /// This round's new rows: the delta.
    Delta,
    /// Old and new.
    All,
}

/// A value a step reads: a constant, or a variable bound by an earlier step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KeyPart {
    Const(ConstVal),
    Var(u16),
}

/// `(column, variable)`: set the variable from that column of the step's row.
pub(crate) type Bind = (u8, u16);

/// `(column, part)`: after the step's binds, that column must equal the part.
pub(crate) type Check = (u8, KeyPart);

/// One operation of a rule body, run left to right by exec (DA.14a).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Every row of `pred` (nothing bound): the identity permutation.
    Scan {
        pred: PredId,
        index: usize,
        perm: Vec<u8>,
        binds: Vec<Bind>,
        checks: Vec<Check>,
        src: Src,
    },
    /// The rows of `pred` whose `perm` prefix equals `key`.
    Lookup {
        pred: PredId,
        index: usize,
        perm: Vec<u8>,
        key: Vec<KeyPart>,
        binds: Vec<Bind>,
        checks: Vec<Check>,
        src: Src,
    },
    /// No row of `pred` at any level has the `perm` prefix `key` (the trailing columns
    /// are the atom's wildcards).
    Neg {
        pred: PredId,
        index: usize,
        perm: Vec<u8>,
        key: Vec<KeyPart>,
    },
    Cmp {
        op: CmpOp,
        l: KeyPart,
        r: KeyPart,
    },
    /// Bind `var` to `from` (`var = from`, `var` free).
    Assign {
        var: u16,
        from: KeyPart,
    },
    /// A builtin call (catalog index `builtin`): `args[c]` is column `c`'s bound value,
    /// `None` when free. `cats` is the category set of an `scc` call, `None` otherwise.
    Call {
        builtin: usize,
        cats: Option<CatSet>,
        args: Vec<Option<KeyPart>>,
        binds: Vec<Bind>,
        checks: Vec<Check>,
        src: Src,
    },
    /// The builtin call yields no row (a `None` argument is a wildcard).
    NegCall {
        builtin: usize,
        cats: Option<CatSet>,
        args: Vec<Option<KeyPart>>,
    },
    /// `reaches(from, to)` per binding: `None` is a free end (column 0 is `from`, 1 is
    /// `to`); a walk from `to` alone runs backward. `depth` bounds the hop count.
    Reach {
        cats: CatSet,
        depth: Option<u32>,
        from: Option<KeyPart>,
        to: Option<KeyPart>,
        binds: Vec<Bind>,
        checks: Vec<Check>,
        src: Src,
    },
    /// `!reaches(from, to)`: no such path (`None` is a wildcard end).
    NegReach {
        cats: CatSet,
        depth: Option<u32>,
        from: Option<KeyPart>,
        to: Option<KeyPart>,
    },
    /// The hoisted multi-source reach: run `seed` to completion over complete input,
    /// collect the distinct values of `seed_var` with their best level, walk once
    /// from all of them (backward from the target end when `backward`), and bind
    /// `bind` to each reached node. `slot` names the seed set exec keeps (DA.14a); the
    /// variants of one rule share it.
    ReachMulti {
        slot: usize,
        cats: CatSet,
        depth: Option<u32>,
        seed: Vec<Step>,
        seed_var: u16,
        backward: bool,
        bind: u16,
        src: Src,
    },
}

/// One semi-naive variant of a rule.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Variant {
    /// The body literal (an index into `CRule::body`) that reads `Src::Delta`; for a
    /// hoisted composite, its `reaches` literal. `None`: everything reads `All`, run
    /// only in the first round of the first level.
    pub(crate) delta: Option<usize>,
    pub(crate) steps: Vec<Step>,
    /// The head row a binding emits; for an aggregate rule, the projected binding.
    pub(crate) head: Vec<KeyPart>,
}

/// One rule's variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RulePlan {
    /// `CRule::index`.
    pub(crate) rule: usize,
    pub(crate) variants: Vec<Variant>,
}

/// One stratum's plan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StratumPlan {
    pub(crate) rules: Vec<RulePlan>,
    /// Every `(predicate, permutation)` a step reads, plus each read or defined
    /// predicate's identity: sorted, deduplicated. A step's `index` is a position here
    /// until [`StratumPlan::resolve`].
    pub(crate) indexes: Vec<(PredId, Vec<u8>)>,
    /// The ReachMulti slots the plan uses: `0..multi_slots`.
    pub(crate) multi_slots: usize,
    /// An aggregate stratum's spec over its one rule's projected bindings.
    pub(crate) agg: Option<AggSpec>,
}

impl StratumPlan {
    /// Rewrite every step's `index` from its position in `indexes` to `map[position]`,
    /// the relation's own index number for that permutation (seeds included). A
    /// position past `map` is left as it is.
    pub(crate) fn resolve(&mut self, map: &[usize]) {
        for v in self.rules.iter_mut().flat_map(|r| r.variants.iter_mut()) {
            resolve_indexes(&mut v.steps, map);
        }
    }
}

/// An aggregate rule's body plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AggPlan {
    /// Every step `Src::All`.
    pub(crate) steps: Vec<Step>,
    /// The projected binding: the group parts, then the distinct aggregated variables.
    pub(crate) project: Vec<KeyPart>,
    pub(crate) spec: AggSpec,
    /// As [`StratumPlan::indexes`].
    pub(crate) indexes: Vec<(PredId, Vec<u8>)>,
    pub(crate) multi_slots: usize,
}

/// Plan stratum `stratum` of `an`. `size` gives a relation's row count where known;
/// the stratum's own predicates read as unknown. An unknown stratum plans nothing.
pub(crate) fn plan_stratum(
    an: &Analysis,
    catalog: &Catalog,
    stratum: usize,
    size: &dyn Fn(PredId) -> Option<usize>,
) -> StratumPlan {
    let Some(st) = an.strata.get(stratum) else { return StratumPlan::default() };
    let own = st.preds.as_slice();
    let masked = |p: PredId| if own.contains(&p) { None } else { size(p) };
    let mut planner = Planner { an, catalog, own, slots: 0 };
    let mut out = StratumPlan::default();
    for rule in an.rules.iter().filter(|r| own.contains(&r.head)) {
        if rule.aggs.is_empty() {
            out.rules.push(planner.rule_plan(rule, &masked));
        } else {
            let (steps, project, spec) = planner.aggregate(rule, &masked);
            out.agg = Some(spec);
            let variant = Variant { delta: None, steps, head: project };
            out.rules.push(RulePlan { rule: rule.index, variants: vec![variant] });
        }
    }
    out.multi_slots = planner.slots;
    let lists = out.rules.iter_mut().flat_map(|r| r.variants.iter_mut()).map(|v| &mut v.steps);
    out.indexes = assign_indexes(an, lists, Some(own));
    out
}

/// Plan aggregate rule `rule` (a `CRule::index`): `None` when there is no such rule or
/// it has no aggregate.
pub(crate) fn plan_aggregate(
    an: &Analysis,
    catalog: &Catalog,
    rule: usize,
    size: &dyn Fn(PredId) -> Option<usize>,
) -> Option<AggPlan> {
    let r = an.rules.iter().find(|r| r.index == rule).filter(|r| !r.aggs.is_empty())?;
    let own: &[PredId] = an
        .preds
        .get(r.head as usize)
        .and_then(|p| an.strata.get(p.stratum))
        .map_or(&[], |s| s.preds.as_slice());
    let masked = |p: PredId| if own.contains(&p) { None } else { size(p) };
    let mut planner = Planner { an, catalog, own, slots: 0 };
    let (mut steps, project, spec) = planner.aggregate(r, &masked);
    let indexes = assign_indexes(an, std::iter::once(&mut steps), Some(own));
    Some(AggPlan { steps, project, spec, indexes, multi_slots: planner.slots })
}

/// The explain plan of rule `rule` (a `CRule::index`) with `bound[v]` marking the
/// variables bound before the body (the head's, for a row): every step `Src::All`, no
/// hoist, and the permutations the steps read (sorted, deduplicated; a step's `index`
/// is a position in that list). No such rule plans nothing.
pub(crate) fn plan_bound(
    an: &Analysis,
    catalog: &Catalog,
    rule: usize,
    bound: &[bool],
) -> (Vec<Step>, Vec<(PredId, Vec<u8>)>) {
    let Some(r) = an.rules.iter().find(|r| r.index == rule) else {
        return (Vec::new(), Vec::new());
    };
    let mut b: Vec<bool> =
        (0..usize::from(r.nvars)).map(|v| bound.get(v).copied().unwrap_or(false)).collect();
    let order = sips_order(r, &an.preds, catalog, &b, None, &|_| None);
    let mut planner = Planner { an, catalog, own: &[], slots: 0 };
    let (mut steps, _) = planner.body(r, &order, &mut b, &|_| Src::All, false);
    let indexes = assign_indexes(an, std::iter::once(&mut steps), None);
    (steps, indexes)
}

/// Rewrite every relation step's `index` (seeds included) from a position in the
/// plan's index list to `map[position]`; a position past `map` is left as it is.
pub(crate) fn resolve_indexes(steps: &mut [Step], map: &[usize]) {
    visit(steps, &mut |s| {
        if let Step::Scan { index, .. } | Step::Lookup { index, .. } | Step::Neg { index, .. } = s
            && let Some(&to) = map.get(*index)
        {
            *index = to;
        }
    });
}

/// Apply `f` to every step, then to the steps of its seed.
fn visit(steps: &mut [Step], f: &mut dyn FnMut(&mut Step)) {
    for s in steps {
        f(s);
        if let Step::ReachMulti { seed, .. } = s {
            visit(seed, f);
        }
    }
}

/// The variant-`delta` source of body literal `j`.
fn assign(j: usize, delta: usize) -> Src {
    match j.cmp(&delta) {
        std::cmp::Ordering::Less => Src::Old,
        std::cmp::Ordering::Equal => Src::Delta,
        std::cmp::Ordering::Greater => Src::All,
    }
}

/// Collect the permutations the steps read (with, given `identity_of`, the identity
/// of every read predicate and of those listed), sort and deduplicate them, and set
/// each relation step's `index` to its permutation's position.
fn assign_indexes<'s>(
    an: &Analysis,
    lists: impl Iterator<Item = &'s mut Vec<Step>>,
    identity_of: Option<&[PredId]>,
) -> Vec<(PredId, Vec<u8>)> {
    let lists: Vec<&mut Vec<Step>> = lists.collect();
    let mut out: Vec<(PredId, Vec<u8>)> = Vec::new();
    for steps in &lists {
        let mut steps = (*steps).clone();
        visit(&mut steps, &mut |s| {
            if let Step::Scan { pred, perm, .. }
            | Step::Lookup { pred, perm, .. }
            | Step::Neg { pred, perm, .. } = s
            {
                out.push((*pred, perm.clone()));
            }
        });
    }
    if let Some(own) = identity_of {
        let read: Vec<PredId> = out.iter().map(|(p, _)| *p).chain(own.iter().copied()).collect();
        for p in read {
            let arity = an.preds.get(p as usize).map_or(0, |i| i.arity);
            out.push((p, (0..arity).map(col).collect()));
        }
    }
    out.sort();
    out.dedup();
    for steps in lists {
        visit(steps, &mut |s| {
            if let Step::Scan { pred, perm, index, .. }
            | Step::Lookup { pred, perm, index, .. }
            | Step::Neg { pred, perm, index, .. } = s
                && let Ok(at) = out.binary_search_by(|(p, q)| (p, q).cmp(&(pred, perm)))
            {
                *index = at;
            }
        });
    }
    out
}

/// A column number as the plan stores it. Relation permutations are `u8` (DA.7); a
/// wider predicate saturates at 255.
fn col(c: usize) -> u8 {
    u8::try_from(c).unwrap_or(u8::MAX)
}

/// A constant, or a variable already bound. `_` is never bound.
fn is_bound(t: &CTerm, bound: &[bool]) -> bool {
    match t {
        CTerm::Const(_) => true,
        CTerm::Var(v) => bound.get(usize::from(*v)).copied().unwrap_or(false),
        CTerm::Wild => false,
    }
}

/// The term as a part; `None` for `_`.
fn part(t: &CTerm) -> Option<KeyPart> {
    match t {
        CTerm::Const(c) => Some(KeyPart::Const(c.clone())),
        CTerm::Var(v) => Some(KeyPart::Var(*v)),
        CTerm::Wild => None,
    }
}

/// The term as a part when bound, `None` when free.
fn bound_part(t: Option<&CTerm>, bound: &[bool]) -> Option<KeyPart> {
    t.filter(|t| is_bound(t, bound)).and_then(part)
}

fn mark(v: u16, bound: &mut [bool]) {
    if let Some(b) = bound.get_mut(usize::from(v)) {
        *b = true;
    }
}

/// Mark every variable among `terms`.
fn mark_all(terms: &[CTerm], bound: &mut [bool]) {
    for t in terms {
        if let CTerm::Var(v) = t {
            mark(*v, bound);
        }
    }
}

/// Mark every variable `lit` mentions.
fn mark_lit(lit: &CLit, out: &mut [bool]) {
    match lit {
        CLit::Pos(_, terms, _) | CLit::Neg(_, terms, _) => mark_all(terms, out),
        CLit::Cmp(_, l, r, _) => mark_all(&[l.clone(), r.clone()], out),
    }
}

/// The binds and checks of `cols` (the free columns, in column order): a variable's
/// first free column binds it, a repeat checks against it.
fn free_columns(terms: &[CTerm], cols: &[usize]) -> (Vec<Bind>, Vec<Check>) {
    let (mut binds, mut checks): (Vec<Bind>, Vec<Check>) = (Vec::new(), Vec::new());
    for &c in cols {
        if let Some(CTerm::Var(v)) = terms.get(c) {
            if binds.iter().any(|(_, b)| b == v) {
                checks.push((col(c), KeyPart::Var(*v)));
            } else {
                binds.push((col(c), *v));
            }
        }
    }
    (binds, checks)
}

/// The bound columns (in column order), their key parts, and the permutation that
/// lists them first, then the rest in column order.
fn keyed(terms: &[CTerm], bound: &[bool]) -> (Vec<KeyPart>, Vec<u8>, Vec<usize>) {
    let (on, off): (Vec<usize>, Vec<usize>) =
        (0..terms.len()).partition(|&c| is_bound(&terms[c], bound));
    let key = on.iter().filter_map(|&c| part(&terms[c])).collect();
    let perm = on.iter().chain(&off).map(|&c| col(c)).collect();
    (key, perm, off)
}

/// A positive relation atom: a Scan when nothing is bound, else a Lookup.
fn relation(pred: PredId, terms: &[CTerm], bound: &mut [bool], src: Src) -> Step {
    let (key, perm, free) = keyed(terms, bound);
    let (binds, checks) = free_columns(terms, &free);
    mark_all(terms, bound);
    if key.is_empty() {
        Step::Scan { pred, index: 0, perm, binds, checks, src }
    } else {
        Step::Lookup { pred, index: 0, perm, key, binds, checks, src }
    }
}

/// A comparison under `bound`; `None` for one with a `_` side.
fn comparison(op: CmpOp, l: &CTerm, r: &CTerm, bound: &mut [bool]) -> Option<Step> {
    let (lp, rp) = (part(l)?, part(r)?);
    let (lb, rb) = (is_bound(l, bound), is_bound(r, bound));
    if op == CmpOp::Eq {
        match (l, r) {
            (CTerm::Var(v), _) if !lb && rb => {
                mark(*v, bound);
                return Some(Step::Assign { var: *v, from: rp });
            }
            (_, CTerm::Var(v)) if lb && !rb => {
                mark(*v, bound);
                return Some(Step::Assign { var: *v, from: lp });
            }
            _ => {}
        }
    }
    // Both sides bound; a free one here is a program analysis rejects (DA.5b's safety).
    Some(Step::Cmp { op, l: lp, r: rp })
}

/// The hop bound of a `reaches/4` call: its depth constant (negative reads as 0).
fn depth(terms: &[CTerm]) -> Option<u32> {
    match terms.get(3) {
        Some(CTerm::Const(ConstVal::Int(n))) => {
            Some(u32::try_from(*n).unwrap_or(if *n < 0 { 0 } else { u32::MAX }))
        }
        _ => None,
    }
}

/// The head row: one part per head term (an aggregate's `_` slot has none).
fn head_parts(rule: &CRule) -> Vec<KeyPart> {
    rule.head_terms.iter().filter_map(part).collect()
}

/// The variables the head reads: its terms' and its aggregates'.
fn head_vars(rule: &CRule) -> Vec<bool> {
    let mut out = vec![false; usize::from(rule.nvars)];
    mark_all(&rule.head_terms, &mut out);
    for (_, vars) in &rule.aggs {
        for v in vars {
            mark(*v, &mut out);
        }
    }
    out
}

/// The planning state of one stratum (or one aggregate / explain plan).
struct Planner<'a> {
    an: &'a Analysis,
    catalog: &'a Catalog,
    /// The stratum being planned: a positive atom of one of these reads input that is
    /// still growing, so it can never feed a hoisted seed.
    own: &'a [PredId],
    /// The next ReachMulti slot.
    slots: usize,
}

impl Planner<'_> {
    fn kind(&self, p: PredId) -> Option<PredKind> {
        self.an.preds.get(p as usize).map(|i| i.kind)
    }

    fn is_reaches(&self, b: usize) -> bool {
        self.catalog.builtins.get(b).is_some_and(|d| d.kind == BuiltinKind::Reaches)
    }

    /// A positive atom of a relation (IDB or EDB).
    fn is_relation(&self, lit: &CLit) -> bool {
        let CLit::Pos(p, ..) = lit else { return false };
        matches!(self.kind(*p), Some(PredKind::Idb | PredKind::Edb(_)))
    }

    /// A positive relation atom, or a positive `tiered` builtin: its rows can be new
    /// at a level.
    fn delta_capable(&self, lit: &CLit) -> bool {
        match lit {
            CLit::Pos(p, ..) => match self.kind(*p) {
                Some(PredKind::Idb | PredKind::Edb(_)) => true,
                Some(PredKind::Builtin(b)) => {
                    self.catalog.builtins.get(b).is_some_and(|d| d.tiered)
                }
                None => false,
            },
            CLit::Neg(..) | CLit::Cmp(..) => false,
        }
    }

    /// A positive atom of the stratum being planned.
    fn reads_own(&self, lit: &CLit) -> bool {
        matches!(lit, CLit::Pos(p, ..) if self.own.contains(p))
    }

    /// One literal's step under `bound` (which it then updates), reading `src`.
    fn step(&self, lit: &CLit, bound: &mut [bool], src: Src) -> Option<Step> {
        match lit {
            CLit::Pos(p, terms, _) => Some(match self.kind(*p) {
                Some(PredKind::Builtin(b)) => self.call(b, terms, bound, src),
                _ => relation(*p, terms, bound, src),
            }),
            CLit::Neg(p, terms, _) => Some(match self.kind(*p) {
                Some(PredKind::Builtin(b)) if self.is_reaches(b) => Step::NegReach {
                    cats: closure_set(self.catalog, b, terms).unwrap_or(CatSet::All),
                    depth: depth(terms),
                    from: bound_part(terms.first(), bound),
                    to: bound_part(terms.get(1), bound),
                },
                Some(PredKind::Builtin(b)) => Step::NegCall {
                    builtin: b,
                    cats: closure_set(self.catalog, b, terms),
                    args: terms.iter().map(|t| bound_part(Some(t), bound)).collect(),
                },
                _ => {
                    let (key, perm, _) = keyed(terms, bound);
                    Step::Neg { pred: *p, index: 0, perm, key }
                }
            }),
            CLit::Cmp(op, l, r, _) => comparison(*op, l, r, bound),
        }
    }

    /// A positive builtin: a per-binding Reach, or a Call.
    fn call(&self, b: usize, terms: &[CTerm], bound: &mut [bool], src: Src) -> Step {
        let cats = closure_set(self.catalog, b, terms);
        let reach = self.is_reaches(b);
        // A reaches call's columns past the two ends are constants.
        let span = if reach { terms.len().min(2) } else { terms.len() };
        let free: Vec<usize> = (0..span).filter(|&c| !is_bound(&terms[c], bound)).collect();
        let (binds, checks) = free_columns(terms, &free);
        let step = if reach {
            Step::Reach {
                cats: cats.unwrap_or(CatSet::All),
                depth: depth(terms),
                from: bound_part(terms.first(), bound),
                to: bound_part(terms.get(1), bound),
                binds,
                checks,
                src,
            }
        } else {
            let args = terms.iter().map(|t| bound_part(Some(t), bound)).collect();
            Step::Call { builtin: b, cats, args, binds, checks, src }
        };
        mark_all(terms, bound);
        step
    }

    /// The ReachMulti `lit` hoists `steps` into, when it meets the hoist condition
    /// under `bound`, with `live` the variables read after it.
    fn hoist(
        &mut self,
        lit: &CLit,
        bound: &[bool],
        live: &[bool],
        steps: &mut Vec<Step>,
    ) -> Option<Step> {
        let CLit::Pos(p, terms, _) = lit else { return None };
        let Some(PredKind::Builtin(b)) = self.kind(*p) else { return None };
        if !self.is_reaches(b) || steps.is_empty() {
            return None;
        }
        let (x, y) = (terms.first()?, terms.get(1)?);
        let (source, other, backward) = match (is_bound(x, bound), is_bound(y, bound)) {
            (true, false) => (x, y, false),
            (false, true) => (y, x, true),
            _ => return None,
        };
        let (CTerm::Var(seed_var), CTerm::Var(bind)) = (source, other) else { return None };
        if bound.iter().zip(live).any(|(b, l)| *b && *l) {
            return None;
        }
        let slot = self.slots;
        self.slots += 1;
        Some(Step::ReachMulti {
            slot,
            cats: closure_set(self.catalog, b, terms).unwrap_or(CatSet::All),
            depth: depth(terms),
            seed: std::mem::take(steps),
            seed_var: *seed_var,
            backward,
            bind: *bind,
            src: Src::All,
        })
    }

    /// Plan the literals `order` (indexes into `rule.body`) from `bound` (updated),
    /// literal `i` reading `src(i)`. With `hoist`, a `reaches` that meets the hoist
    /// condition folds every step before it into a ReachMulti; the second result is
    /// then the position in `order` of the last literal folded.
    fn body(
        &mut self,
        rule: &CRule,
        order: &[usize],
        bound: &mut [bool],
        src: &dyn Fn(usize) -> Src,
        hoist: bool,
    ) -> (Vec<Step>, Option<usize>) {
        // live[k]: the variables a literal after position k, or the head, reads.
        let mut live: Vec<Vec<bool>> = Vec::new();
        if hoist {
            let mut acc = head_vars(rule);
            live = vec![Vec::new(); order.len()];
            for k in (0..order.len()).rev() {
                live[k] = acc.clone();
                if let Some(lit) = rule.body.get(order[k]) {
                    mark_lit(lit, &mut acc);
                }
            }
        }
        let mut steps = Vec::new();
        let mut folded = None;
        let mut complete = true;
        for (k, &i) in order.iter().enumerate() {
            let Some(lit) = rule.body.get(i) else { continue };
            if hoist
                && complete
                && let Some(multi) = self.hoist(lit, bound, &live[k], &mut steps)
            {
                if let CLit::Pos(_, terms, _) = lit {
                    mark_all(terms, bound);
                }
                steps.push(multi);
                folded = Some(k);
                continue;
            }
            complete &= !self.reads_own(lit);
            if let Some(step) = self.step(lit, bound, src(i)) {
                steps.push(step);
            }
        }
        (steps, folded)
    }

    /// One rule's variants.
    fn rule_plan(&mut self, rule: &CRule, size: &dyn Fn(PredId) -> Option<usize>) -> RulePlan {
        let (n, len) = (usize::from(rule.nvars), rule.body.len());
        let (an, catalog) = (self.an, self.catalog);
        let preds = &an.preds;
        let head = head_parts(rule);
        let base_order = sips_order(rule, preds, catalog, &[], None, size);
        let (base, folded) = self.body(rule, &base_order, &mut vec![false; n], &|_| Src::All, true);
        let mut variants = Vec::new();
        let Some(k) = folded else {
            variants.push(Variant { delta: None, steps: base, head: head.clone() });
            let capable: Vec<usize> =
                (0..len).filter(|&d| self.delta_capable(&rule.body[d])).collect();
            for d in capable {
                let first = self.is_relation(&rule.body[d]).then_some(d);
                let order = sips_order(rule, preds, catalog, &[], first, size);
                let src = move |j: usize| assign(j, d);
                let (steps, _) = self.body(rule, &order, &mut vec![false; n], &src, false);
                variants.push(Variant { delta: Some(d), steps, head: head.clone() });
            }
            return RulePlan { rule: rule.index, variants };
        };
        // The composite: base[0], folding every literal up to base_order[k].
        let r = base_order[k];
        let mut member = vec![false; len];
        let mut cbound = vec![false; n];
        for &i in &base_order[..=k] {
            member[i] = true;
            mark_lit(&rule.body[i], &mut cbound);
        }
        let composite = base.first().cloned();
        variants.push(Variant { delta: None, steps: base, head: head.clone() });
        let mut deltas: Vec<usize> =
            (0..len).filter(|&d| !member[d] && self.delta_capable(&rule.body[d])).collect();
        deltas.push(r);
        deltas.sort_unstable();
        for d in deltas {
            let src = move |j: usize| assign(j, d);
            let first = (d != r && self.is_relation(&rule.body[d])).then_some(d);
            let order: Vec<usize> = sips_order(rule, preds, catalog, &cbound, first, size)
                .into_iter()
                .filter(|&i| !member[i])
                .collect();
            let mut steps: Vec<Step> = composite.iter().cloned().collect();
            if let Some(Step::ReachMulti { src: s, .. }) = steps.first_mut() {
                *s = src(r);
            }
            let (rest, _) = self.body(rule, &order, &mut cbound.clone(), &src, false);
            steps.extend(rest);
            variants.push(Variant { delta: Some(d), steps, head: head.clone() });
        }
        RulePlan { rule: rule.index, variants }
    }

    /// An aggregate rule's body plan, its projection and its spec.
    fn aggregate(
        &mut self,
        rule: &CRule,
        size: &dyn Fn(PredId) -> Option<usize>,
    ) -> (Vec<Step>, Vec<KeyPart>, AggSpec) {
        let order = sips_order(rule, &self.an.preds, self.catalog, &[], None, size);
        let mut bound = vec![false; usize::from(rule.nvars)];
        let (steps, _) = self.body(rule, &order, &mut bound, &|_| Src::All, true);
        let (mut project, mut out_order, mut j) = (Vec::new(), Vec::new(), 0);
        for t in &rule.head_terms {
            match part(t) {
                Some(p) => {
                    out_order.push(OutCol::Group(project.len()));
                    project.push(p);
                }
                // An aggregate's slot: the j-th `_` is aggs[j].
                None => {
                    out_order.push(OutCol::Agg(j));
                    j += 1;
                }
            }
        }
        let group = project.len();
        let mut aggregated: Vec<u16> = Vec::new();
        let mut ops = Vec::with_capacity(rule.aggs.len());
        for (op, vars) in &rule.aggs {
            let cols = vars
                .iter()
                .map(|v| {
                    group
                        + aggregated.iter().position(|a| a == v).unwrap_or_else(|| {
                            aggregated.push(*v);
                            aggregated.len() - 1
                        })
                })
                .collect();
            ops.push((*op, cols));
        }
        project.extend(aggregated.into_iter().map(KeyPart::Var));
        (steps, project, AggSpec { group, ops, out_order })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyse;
    use crate::ir::{
        AggOp, Atom, BindReq, BuiltinDecl, ColType, HeadArg, Literal, Program, Rule, Term,
    };
    use glia_activation::profile::Registries;
    use glia_core::{CellTypeId, EdgeCategoryId, NodeKindId};

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

    /// `in_scope`'s catalog index: after the eight core builtins.
    const IN_SCOPE: usize = 8;
    /// `starts_with/2`'s catalog index.
    const STARTS_WITH: usize = 5;

    fn catalog() -> Catalog {
        Catalog::core(REG, &[EdgeCategoryId(1), EdgeCategoryId(3)]).with_builtins(vec![
            BuiltinDecl {
                name: "in_scope".into(),
                cols: vec![ColType::Node, ColType::Str],
                bind: vec![BindReq::May, BindReq::Must],
                tiered: false,
                kind: BuiltinKind::Domain,
            },
        ])
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

    fn analysed(rules: Vec<Rule>) -> (Analysis, Catalog) {
        let c = catalog();
        let mut p = Program::new();
        p.add_source("rules");
        p.rules = rules;
        match analyse(&p, &c) {
            Ok(a) => (a, c),
            Err(e) => panic!("expected an analysis, got: {e}"),
        }
    }

    fn pid(an: &Analysis, name: &str) -> PredId {
        let p = an.preds.iter().position(|p| p.name == name);
        p.map(|p| p as PredId).unwrap_or_else(|| panic!("no predicate {name}"))
    }

    fn var(an: &Analysis, rule: usize, name: &str) -> u16 {
        let v = an.rules[rule].var_names.iter().position(|n| n == name);
        v.map(|v| v as u16).unwrap_or_else(|| panic!("no variable {name} in rule {rule}"))
    }

    fn stratum(an: &Analysis, name: &str) -> usize {
        an.preds[pid(an, name) as usize].stratum
    }

    fn plan_of(an: &Analysis, c: &Catalog, head: &str) -> StratumPlan {
        plan_stratum(an, c, stratum(an, head), &|_| None)
    }

    fn ix(indexes: &[(PredId, Vec<u8>)], pred: PredId, perm: &[u8]) -> usize {
        let at = indexes.iter().position(|(p, q)| *p == pred && q == perm);
        at.unwrap_or_else(|| panic!("no index ({pred}, {perm:?}) in {indexes:?}"))
    }

    fn cs(x: &str) -> KeyPart {
        KeyPart::Const(ConstVal::Str(x.into()))
    }

    fn rule_plan(plan: &StratumPlan, rule: usize) -> &RulePlan {
        let r = plan.rules.iter().find(|r| r.rule == rule);
        r.unwrap_or_else(|| panic!("no plan for rule {rule}"))
    }

    fn variant(rp: &RulePlan, delta: Option<usize>) -> &Variant {
        let found = rp.variants.iter().find(|v| v.delta == delta);
        found.unwrap_or_else(|| panic!("no variant {delta:?} in {:?}", rp.variants))
    }

    fn deltas(rp: &RulePlan) -> Vec<Option<usize>> {
        rp.variants.iter().map(|v| v.delta).collect()
    }

    fn src_of(step: &Step) -> Option<Src> {
        match step {
            Step::Scan { src, .. }
            | Step::Lookup { src, .. }
            | Step::Call { src, .. }
            | Step::Reach { src, .. }
            | Step::ReachMulti { src, .. } => Some(*src),
            _ => None,
        }
    }

    /// `v(a, b) :- edge(a, b, c), carry_category(c), in_scope(a, "cli"), in_scope(b, "store").`
    fn forbid() -> Rule {
        rule(
            "v",
            vec![v("a"), v("b")],
            vec![
                pos("edge", vec![v("a"), v("b"), v("c")]),
                pos("carry_category", vec![v("c")]),
                pos("in_scope", vec![v("a"), s("cli")]),
                pos("in_scope", vec![v("b"), s("store")]),
            ],
        )
    }

    #[test]
    fn forbid_shape_orders_bound_first() {
        let (an, c) = analysed(vec![forbid()]);
        let plan = plan_of(&an, &c, "v");
        let (a, b, cv) = (var(&an, 0, "a"), var(&an, 0, "b"), var(&an, 0, "c"));
        let (vp, edge, carry) = (pid(&an, "v"), pid(&an, "edge"), pid(&an, "carry_category"));
        let rp = rule_plan(&plan, 0);
        let none = variant(rp, None);
        assert_eq!(
            none.steps,
            vec![
                Step::Call {
                    builtin: IN_SCOPE,
                    cats: None,
                    args: vec![None, Some(cs("cli"))],
                    binds: vec![(0, a)],
                    checks: vec![],
                    src: Src::All
                },
                Step::Lookup {
                    pred: edge,
                    index: ix(&plan.indexes, edge, &[0, 1, 2]),
                    perm: vec![0, 1, 2],
                    key: vec![KeyPart::Var(a)],
                    binds: vec![(1, b), (2, cv)],
                    checks: vec![],
                    src: Src::All
                },
                Step::Lookup {
                    pred: carry,
                    index: ix(&plan.indexes, carry, &[0]),
                    perm: vec![0],
                    key: vec![KeyPart::Var(cv)],
                    binds: vec![],
                    checks: vec![],
                    src: Src::All
                },
                Step::Call {
                    builtin: IN_SCOPE,
                    cats: None,
                    args: vec![Some(KeyPart::Var(b)), Some(cs("store"))],
                    binds: vec![],
                    checks: vec![],
                    src: Src::All
                },
            ]
        );
        assert_eq!(none.head, vec![KeyPart::Var(a), KeyPart::Var(b)]);
        // edge and carry_category are delta-capable; in_scope is untiered.
        assert_eq!(deltas(rp), [None, Some(0), Some(1)]);
        // The carry_category delta runs first and edge is then looked up by c.
        let by_carry = variant(rp, Some(1));
        assert!(
            matches!(&by_carry.steps[0], Step::Scan { pred, src: Src::Delta, .. } if *pred == carry)
        );
        assert!(matches!(
            &by_carry.steps[1],
            Step::Lookup { pred, perm, src: Src::Old, .. } if *pred == edge && perm == &[2, 0, 1]
        ));
        // Sorted, deduplicated, with every read and defined predicate's identity.
        assert_eq!(
            plan.indexes,
            vec![(vp, vec![0, 1]), (edge, vec![0, 1, 2]), (edge, vec![2, 0, 1]), (carry, vec![0])]
        );
        assert_eq!((plan.multi_slots, &plan.agg), (0, &None));
    }

    #[test]
    fn variants_and_sources() {
        let (an, c) = analysed(vec![
            rule("tc", vec![v("x"), v("y")], vec![pos("edge", vec![v("x"), v("y"), s("CALLS")])]),
            rule(
                "tc",
                vec![v("x"), v("z")],
                vec![
                    pos("tc", vec![v("x"), v("y")]),
                    pos("edge", vec![v("y"), v("z"), s("CALLS")]),
                ],
            ),
        ]);
        assert!(an.strata[stratum(&an, "tc")].recursive);
        let plan = plan_of(&an, &c, "tc");
        let (tc, edge) = (pid(&an, "tc"), pid(&an, "edge"));
        let (x, y, z) = (var(&an, 1, "x"), var(&an, 1, "y"), var(&an, 1, "z"));
        let rp = rule_plan(&plan, 1);
        assert_eq!(deltas(rp), [None, Some(0), Some(1)]);

        // Literal 0: tc's delta first, then edge (All) by y.
        let v0 = variant(rp, Some(0));
        assert_eq!(
            v0.steps,
            vec![
                Step::Scan {
                    pred: tc,
                    index: ix(&plan.indexes, tc, &[0, 1]),
                    perm: vec![0, 1],
                    binds: vec![(0, x), (1, y)],
                    checks: vec![],
                    src: Src::Delta
                },
                Step::Lookup {
                    pred: edge,
                    index: ix(&plan.indexes, edge, &[0, 2, 1]),
                    perm: vec![0, 2, 1],
                    key: vec![KeyPart::Var(y), cs("CALLS")],
                    binds: vec![(1, z)],
                    checks: vec![],
                    src: Src::All
                },
            ]
        );
        // Literal 1: edge's delta first, then tc (Old: it comes before in the body) by y.
        let v1 = variant(rp, Some(1));
        assert_eq!(
            v1.steps,
            vec![
                Step::Lookup {
                    pred: edge,
                    index: ix(&plan.indexes, edge, &[2, 0, 1]),
                    perm: vec![2, 0, 1],
                    key: vec![cs("CALLS")],
                    binds: vec![(0, y), (1, z)],
                    checks: vec![],
                    src: Src::Delta
                },
                Step::Lookup {
                    pred: tc,
                    index: ix(&plan.indexes, tc, &[1, 0]),
                    perm: vec![1, 0],
                    key: vec![KeyPart::Var(y)],
                    binds: vec![(0, x)],
                    checks: vec![],
                    src: Src::Old
                },
            ]
        );
        // The first round of the first level reads everything whole.
        assert!(variant(rp, None).steps.iter().all(|s| src_of(s) == Some(Src::All)));
        // The base rule's one EDB literal is its only delta.
        assert_eq!(deltas(rule_plan(&plan, 0)), [None, Some(0)]);
    }

    #[test]
    fn index_choice_dedups() {
        let looked_up = |kind: &str| {
            rule(
                "p",
                vec![v("x")],
                vec![
                    pos("node", vec![v("y"), s(kind)]),
                    pos("edge", vec![v("x"), v("y"), Term::wild()]),
                ],
            )
        };
        let (an, c) = analysed(vec![looked_up("ROUTE"), looked_up("MODULE")]);
        let plan = plan_of(&an, &c, "p");
        let edge = pid(&an, "edge");
        let by_col_1 = plan.indexes.iter().filter(|(p, q)| *p == edge && q == &[1, 0, 2]).count();
        assert_eq!(by_col_1, 1, "{:?}", plan.indexes);
        let at = ix(&plan.indexes, edge, &[1, 0, 2]);
        for r in 0..2 {
            let none = variant(rule_plan(&plan, r), None);
            let hit = matches!(
                &none.steps[1],
                Step::Lookup { pred, index, .. } if *pred == edge && *index == at
            );
            assert!(hit, "{:?}", none.steps);
        }
        let edges: Vec<&Vec<u8>> =
            plan.indexes.iter().filter(|(p, _)| *p == edge).map(|(_, q)| q).collect();
        assert_eq!(edges, [&vec![0, 1, 2], &vec![1, 0, 2]]);
    }

    #[test]
    fn hoist_multi_source() {
        let (an, c) = analysed(vec![rule(
            "hit",
            vec![v("b")],
            vec![
                pos("in_scope", vec![v("a"), s("cli")]),
                pos("reaches", vec![v("a"), v("b")]),
                pos("in_scope", vec![v("b"), s("store")]),
            ],
        )]);
        let plan = plan_of(&an, &c, "hit");
        let (a, b) = (var(&an, 0, "a"), var(&an, 0, "b"));
        let seed = Step::Call {
            builtin: IN_SCOPE,
            cats: None,
            args: vec![None, Some(cs("cli"))],
            binds: vec![(0, a)],
            checks: vec![],
            src: Src::All,
        };
        let multi = |src: Src| Step::ReachMulti {
            slot: 0,
            cats: CatSet::Carry,
            depth: None,
            seed: vec![seed.clone()],
            seed_var: a,
            backward: false,
            bind: b,
            src,
        };
        let after = Step::Call {
            builtin: IN_SCOPE,
            cats: None,
            args: vec![Some(KeyPart::Var(b)), Some(cs("store"))],
            binds: vec![],
            checks: vec![],
            src: Src::All,
        };
        let rp = rule_plan(&plan, 0);
        // The composite is one delta-capable literal at its reaches literal's index.
        assert_eq!(deltas(rp), [None, Some(1)]);
        assert_eq!(variant(rp, None).steps, vec![multi(Src::All), after.clone()]);
        assert_eq!(variant(rp, Some(1)).steps, vec![multi(Src::Delta), after]);
        assert_eq!(plan.multi_slots, 1);
    }

    #[test]
    fn no_hoist_when_source_is_live() {
        let (an, c) = analysed(vec![rule(
            "pair",
            vec![v("a"), v("b")],
            vec![pos("in_scope", vec![v("a"), s("cli")]), pos("reaches", vec![v("a"), v("b")])],
        )]);
        let plan = plan_of(&an, &c, "pair");
        let (a, b) = (var(&an, 0, "a"), var(&an, 0, "b"));
        let rp = rule_plan(&plan, 0);
        assert_eq!(
            variant(rp, None).steps[1],
            Step::Reach {
                cats: CatSet::Carry,
                depth: None,
                from: Some(KeyPart::Var(a)),
                to: None,
                binds: vec![(1, b)],
                checks: vec![],
                src: Src::All
            }
        );
        // reaches is tiered, so the per-binding call is a delta of its own.
        assert_eq!(deltas(rp), [None, Some(1)]);
        assert!(matches!(variant(rp, Some(1)).steps[1], Step::Reach { src: Src::Delta, .. }));
        assert_eq!(plan.multi_slots, 0);
    }

    #[test]
    fn backward_hoist() {
        let (an, c) = analysed(vec![rule(
            "hit",
            vec![v("a")],
            vec![
                pos("in_scope", vec![v("b"), s("store")]),
                pos("reaches", vec![v("a"), v("b"), s("CALLS")]),
            ],
        )]);
        let plan = plan_of(&an, &c, "hit");
        let (a, b) = (var(&an, 0, "a"), var(&an, 0, "b"));
        let none = variant(rule_plan(&plan, 0), None);
        assert_eq!(
            none.steps,
            vec![Step::ReachMulti {
                slot: 0,
                cats: CatSet::Of(vec![EdgeCategoryId(1)]),
                depth: None,
                seed: vec![Step::Call {
                    builtin: IN_SCOPE,
                    cats: None,
                    args: vec![None, Some(cs("store"))],
                    binds: vec![(0, b)],
                    checks: vec![],
                    src: Src::All
                }],
                seed_var: b,
                backward: true,
                bind: a,
                src: Src::All
            }]
        );
    }

    #[test]
    fn negation_with_wildcard() {
        let (an, c) = analysed(vec![rule(
            "v",
            vec![v("r")],
            vec![
                pos("node", vec![v("r"), s("ROUTE")]),
                neg("edge", vec![v("r"), Term::wild(), s("HANDLED_BY")]),
            ],
        )]);
        let plan = plan_of(&an, &c, "v");
        let (r, edge, node) = (var(&an, 0, "r"), pid(&an, "edge"), pid(&an, "node"));
        let rp = rule_plan(&plan, 0);
        assert_eq!(
            variant(rp, None).steps,
            vec![
                Step::Lookup {
                    pred: node,
                    index: ix(&plan.indexes, node, &[1, 0]),
                    perm: vec![1, 0],
                    key: vec![cs("ROUTE")],
                    binds: vec![(0, r)],
                    checks: vec![],
                    src: Src::All
                },
                Step::Neg {
                    pred: edge,
                    index: ix(&plan.indexes, edge, &[0, 2, 1]),
                    perm: vec![0, 2, 1],
                    key: vec![KeyPart::Var(r), cs("HANDLED_BY")]
                },
            ]
        );
        // A negation is never a delta.
        assert_eq!(deltas(rp), [None, Some(0)]);
        assert!(plan.indexes.contains(&(edge, vec![0, 2, 1])));
    }

    #[test]
    fn plan_bound_for_explain() {
        // The forbid shape with its head bound: the in_scope calls are filters, edge a
        // two-column Lookup, every Src All, and only the permutations read are listed.
        let (an, c) = analysed(vec![forbid()]);
        let (a, b, cv) = (var(&an, 0, "a"), var(&an, 0, "b"), var(&an, 0, "c"));
        let (edge, carry) = (pid(&an, "edge"), pid(&an, "carry_category"));
        let mut bound = vec![false; usize::from(an.rules[0].nvars)];
        bound[usize::from(a)] = true;
        bound[usize::from(b)] = true;
        let (steps, indexes) = plan_bound(&an, &c, 0, &bound);
        assert!(steps.iter().all(|s| src_of(s).is_none_or(|s| s == Src::All)));
        assert_eq!(indexes, vec![(edge, vec![0, 1, 2]), (carry, vec![0])]);
        assert_eq!(
            steps[2],
            Step::Lookup {
                pred: edge,
                index: 0,
                perm: vec![0, 1, 2],
                key: vec![KeyPart::Var(a), KeyPart::Var(b)],
                binds: vec![(2, cv)],
                checks: vec![],
                src: Src::All
            }
        );
        assert!(matches!(&steps[3], Step::Lookup { pred, index: 1, .. } if *pred == carry));

        // A permutation the evaluation plan never requested.
        let (an, c) = analysed(vec![
            rule("r", vec![v("z")], vec![pos("node", vec![v("z"), s("ROUTE")])]),
            rule(
                "r",
                vec![v("z")],
                vec![pos("r", vec![v("y")]), pos("edge", vec![v("y"), v("z"), s("CALLS")])],
            ),
        ]);
        let (rp, edge) = (pid(&an, "r"), pid(&an, "edge"));
        let (y, z) = (var(&an, 1, "y"), var(&an, 1, "z"));
        let mut bound = vec![false; usize::from(an.rules[1].nvars)];
        bound[usize::from(z)] = true;
        let (steps, indexes) = plan_bound(&an, &c, 1, &bound);
        assert_eq!(indexes, vec![(rp, vec![0]), (edge, vec![1, 2, 0])]);
        assert_eq!(
            steps,
            vec![
                Step::Lookup {
                    pred: edge,
                    index: 1,
                    perm: vec![1, 2, 0],
                    key: vec![KeyPart::Var(z), cs("CALLS")],
                    binds: vec![(0, y)],
                    checks: vec![],
                    src: Src::All
                },
                Step::Lookup {
                    pred: rp,
                    index: 0,
                    perm: vec![0],
                    key: vec![KeyPart::Var(y)],
                    binds: vec![],
                    checks: vec![],
                    src: Src::All
                },
            ]
        );
        let eval = plan_of(&an, &c, "r");
        assert!(eval.indexes.contains(&(edge, vec![0, 2, 1])), "{:?}", eval.indexes);
        assert!(!eval.indexes.contains(&(edge, vec![1, 2, 0])), "{:?}", eval.indexes);
        // An unknown rule plans nothing.
        assert_eq!(plan_bound(&an, &c, 9, &[]), (Vec::new(), Vec::new()));
    }

    #[test]
    fn aggregate_projects_groups_then_aggregated() {
        let (an, c) = analysed(vec![Rule::new(
            "fan",
            vec![
                v("y").into(),
                s("calls").into(),
                HeadArg::agg(AggOp::Count, &["x"]),
                HeadArg::agg(AggOp::Max, &["x"]),
            ],
            vec![
                pos("edge", vec![v("x"), v("y"), s("CALLS")]),
                pos("node", vec![v("x"), s("FUNCTION")]),
            ],
        )]);
        let (x, y) = (var(&an, 0, "x"), var(&an, 0, "y"));
        let agg =
            plan_aggregate(&an, &c, 0, &|_| None).unwrap_or_else(|| panic!("an aggregate plan"));
        assert_eq!(agg.project, vec![KeyPart::Var(y), cs("calls"), KeyPart::Var(x)]);
        assert_eq!(
            agg.spec,
            AggSpec {
                group: 2,
                ops: vec![(AggOp::Count, vec![2]), (AggOp::Max, vec![2])],
                out_order: vec![OutCol::Group(0), OutCol::Group(1), OutCol::Agg(0), OutCol::Agg(1)],
            }
        );
        assert!(agg.steps.iter().all(|s| src_of(s).is_none_or(|s| s == Src::All)));
        // The stratum plan carries the same body, projection and spec as one variant.
        let plan = plan_of(&an, &c, "fan");
        let rp = rule_plan(&plan, 0);
        assert_eq!(deltas(rp), [None]);
        assert_eq!(rp.variants[0].steps, agg.steps);
        assert_eq!(rp.variants[0].head, agg.project);
        assert_eq!(plan.agg, Some(agg.spec));
        assert_eq!(plan.indexes, agg.indexes);
        assert!(plan_aggregate(&an, &c, 1, &|_| None).is_none());
    }

    #[test]
    fn repeats_assignments_and_negated_builtins() {
        let (an, c) = analysed(vec![
            rule("self_loop", vec![v("x")], vec![pos("edge", vec![v("x"), v("x"), s("CALLS")])]),
            rule("cyc", vec![v("x")], vec![pos("reaches", vec![v("x"), v("x"), s("CALLS")])]),
            rule(
                "w",
                vec![v("x"), v("z")],
                vec![
                    pos("node", vec![v("x"), Term::wild()]),
                    Literal::cmp(CmpOp::Eq, v("z"), v("x")),
                ],
            ),
            rule(
                "quiet",
                vec![v("x")],
                vec![
                    pos("node", vec![v("x"), s("FUNCTION")]),
                    pos("cell", vec![v("x"), s("CODE"), v("t")]),
                    neg("starts_with", vec![v("t"), s("test_")]),
                    neg("reaches", vec![v("x"), Term::wild(), s("CALLS")]),
                ],
            ),
        ]);
        let first = |head: &str, rule: usize| {
            variant(rule_plan(&plan_of(&an, &c, head), rule), None).steps.clone()
        };
        let x0 = var(&an, 0, "x");
        assert!(matches!(
            &first("self_loop", 0)[0],
            Step::Lookup { perm, key, binds, checks, .. }
                if perm == &[2, 0, 1] && key == &[cs("CALLS")] && binds == &[(0, x0)]
                    && checks == &[(1, KeyPart::Var(x0))]
        ));
        let x1 = var(&an, 1, "x");
        assert_eq!(
            first("cyc", 1),
            vec![Step::Reach {
                cats: CatSet::Of(vec![EdgeCategoryId(1)]),
                depth: None,
                from: None,
                to: None,
                binds: vec![(0, x1)],
                checks: vec![(1, KeyPart::Var(x1))],
                src: Src::All
            }]
        );
        let (x2, z2) = (var(&an, 2, "x"), var(&an, 2, "z"));
        let w = first("w", 2);
        // Analysis rejects `x != _` (DA.16), so no comparison here holds a `_`.
        assert_eq!(w.len(), 2, "{w:?}");
        assert_eq!(w[1], Step::Assign { var: z2, from: KeyPart::Var(x2) });
        let (x3, t3) = (var(&an, 3, "x"), var(&an, 3, "t"));
        // `!reaches(x, _, ..)` is a filter once node binds x, so it runs before cell.
        let quiet = first("quiet", 3);
        assert_eq!(
            quiet[1],
            Step::NegReach {
                cats: CatSet::Of(vec![EdgeCategoryId(1)]),
                depth: None,
                from: Some(KeyPart::Var(x3)),
                to: None
            }
        );
        assert_eq!(
            quiet[3],
            Step::NegCall {
                builtin: STARTS_WITH,
                cats: None,
                args: vec![Some(KeyPart::Var(t3)), Some(cs("test_"))]
            }
        );
    }

    #[test]
    fn nested_hoists_share_slots_across_variants() {
        let (an, c) = analysed(vec![rule(
            "hit",
            vec![v("c")],
            vec![
                pos("in_scope", vec![v("a"), s("cli")]),
                pos("reaches", vec![v("a"), v("b"), s("CALLS")]),
                pos("reaches", vec![v("b"), v("c"), s("USES")]),
            ],
        )]);
        let plan = plan_of(&an, &c, "hit");
        let (a, b, cv) = (var(&an, 0, "a"), var(&an, 0, "b"), var(&an, 0, "c"));
        let inner = Step::ReachMulti {
            slot: 0,
            cats: CatSet::Of(vec![EdgeCategoryId(1)]),
            depth: None,
            seed: vec![Step::Call {
                builtin: IN_SCOPE,
                cats: None,
                args: vec![None, Some(cs("cli"))],
                binds: vec![(0, a)],
                checks: vec![],
                src: Src::All,
            }],
            seed_var: a,
            backward: false,
            bind: b,
            src: Src::All,
        };
        let outer = |src: Src| Step::ReachMulti {
            slot: 1,
            cats: CatSet::Of(vec![EdgeCategoryId(3)]),
            depth: None,
            seed: vec![inner.clone()],
            seed_var: b,
            backward: false,
            bind: cv,
            src,
        };
        let rp = rule_plan(&plan, 0);
        assert_eq!(deltas(rp), [None, Some(2)]);
        assert_eq!(variant(rp, None).steps, vec![outer(Src::All)]);
        assert_eq!(variant(rp, Some(2)).steps, vec![outer(Src::Delta)]);
        assert_eq!(plan.multi_slots, 2);
    }

    #[test]
    fn hoist_needs_complete_input_and_resolve_reaches_seeds() {
        // r's own atom feeds the call: the stratum is still growing, so no hoist.
        let (an, c) = analysed(vec![
            rule("r", vec![v("b")], vec![pos("node", vec![v("b"), s("ROUTE")])]),
            rule(
                "r",
                vec![v("b")],
                vec![pos("r", vec![v("a")]), pos("reaches", vec![v("a"), v("b")])],
            ),
            rule("seed", vec![v("a")], vec![pos("node", vec![v("a"), s("MODULE")])]),
            rule(
                "hit",
                vec![v("b")],
                vec![
                    pos("seed", vec![v("a")]),
                    pos("reaches", vec![v("a"), v("b")]),
                    pos("node", vec![v("b"), Term::wild()]),
                ],
            ),
        ]);
        let recursive = plan_of(&an, &c, "r");
        let steps = &variant(rule_plan(&recursive, 1), None).steps;
        assert!(matches!(steps[1], Step::Reach { from: Some(_), .. }), "{steps:?}");
        assert_eq!(recursive.multi_slots, 0);
        // A lower stratum is complete: the seed folds, and node(b, _) is a delta.
        let mut plan = plan_of(&an, &c, "hit");
        let rp = rule_plan(&plan, 3);
        assert_eq!(deltas(rp), [None, Some(1), Some(2)]);
        let by_node = variant(rp, Some(2));
        assert!(matches!(by_node.steps[0], Step::ReachMulti { slot: 0, src: Src::Old, .. }));
        assert!(matches!(by_node.steps[1], Step::Lookup { src: Src::Delta, .. }));
        // resolve maps every index, inside seeds too.
        let before = plan.clone();
        let map: Vec<usize> = (0..plan.indexes.len()).map(|i| i + 100).collect();
        plan.resolve(&map);
        let mut seen = 0;
        let mut a = before.rules.clone();
        let mut b = plan.rules.clone();
        for (x, y) in a.iter_mut().zip(b.iter_mut()) {
            for (vx, vy) in x.variants.iter_mut().zip(y.variants.iter_mut()) {
                let mut old = Vec::new();
                visit(&mut vx.steps, &mut |s| {
                    if let Step::Scan { index, .. }
                    | Step::Lookup { index, .. }
                    | Step::Neg { index, .. } = s
                    {
                        old.push(*index);
                    }
                });
                let mut new = Vec::new();
                visit(&mut vy.steps, &mut |s| {
                    if let Step::Scan { index, .. }
                    | Step::Lookup { index, .. }
                    | Step::Neg { index, .. } = s
                    {
                        new.push(*index);
                    }
                });
                assert_eq!(new, old.iter().map(|i| i + 100).collect::<Vec<_>>());
                seen += old.len();
            }
        }
        // The seed Lookup of seed(a) appears in all three variants, plus node's Lookups.
        assert!(seen >= 6, "{seen}");
    }

    #[test]
    fn own_sizes_are_unknown_and_plans_are_deterministic() {
        let (an, c) = analysed(vec![
            rule("t", vec![v("x"), v("y")], vec![pos("edge", vec![v("x"), v("y"), s("CALLS")])]),
            rule(
                "t",
                vec![v("x"), v("y")],
                vec![
                    pos("t", vec![v("x"), v("z")]),
                    pos("edge", vec![v("z"), v("y"), Term::wild()]),
                ],
            ),
        ]);
        let (t, edge) = (pid(&an, "t"), pid(&an, "edge"));
        // t reads as size 0 to the caller, but it is still growing: the smaller known
        // relation, edge, runs first.
        let size = |p: PredId| Some(if p == t { 0 } else { 10 });
        let plan = plan_stratum(&an, &c, stratum(&an, "t"), &size);
        let none = variant(rule_plan(&plan, 1), None);
        assert!(
            matches!(&none.steps[0], Step::Scan { pred, .. } if *pred == edge),
            "{:?}",
            none.steps
        );
        assert_eq!(plan, plan_stratum(&an, &c, stratum(&an, "t"), &size));
        assert_eq!(plan_stratum(&an, &c, 7, &size), StratumPlan::default());
    }
}
