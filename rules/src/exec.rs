//! The step interpreter behind evaluation, aggregates and explain (DA.14a).
//!
//! [`run_body`] runs one plan's steps (DA.10) depth-first and hands every complete
//! binding to the caller: DA.14b pushes head rows (or an aggregate's projected
//! bindings) from it, DA.15 picks a witness among them. The walk is an explicit
//! step-index loop over one bindings array, never recursion per step, so a deep body
//! does not grow the stack: each step's matching rows are collected into a frame (an
//! alternative is a full row in relation column order with its tier and provenance),
//! then taken one at a time. A step reads only variables earlier steps bound, so
//! popping a frame resets just the variables its step binds.
//!
//! **What a step reads (the Src -> View mapping DA.14b states).** In level L's first
//! round `Delta` is level L, `Old` the levels below L (none at FACT) and `All` the
//! levels up to L; in later rounds `Delta` is level L's `recent` run, `Old` the stable
//! runs up to L and `All` both. Builtin, `reaches` and hoisted rows are complete
//! input, so they read as stable rows. Explain reads every level up to L below its
//! height bound. A negation (`Neg`, `NegCall`, `NegReach`) reads the COMPLETE relation
//! at every level and adds no tier (Q5).
//!
//! **Levels and heights.** A binding's level is the weakest of its positive rows'
//! tiers (node and attribute rows are FACT, builtin rows carry theirs); its height is
//! the highest of its relation rows' heights (EDB rows are 0).
//!
//! **Builtins.** A domain or string builtin is called through [`Builtin::call`] once
//! per (builtin, bound arguments) and its interned rows kept in the evaluation's
//! [`Memo`], a `Lookup` that is never iterated; an untiered builtin's rows read FACT
//! whatever it reports (the plan never makes it a delta, so a weaker row would be
//! lost). `scc` reads the relation DA.14b materialised for its category set from
//! `Closure::scc_rows`. A per-binding `reaches` is `Closure::reach` (memoised there);
//! one with both ends free is `Closure::reach_all`, once per run; a hoisted
//! `ReachMulti` runs its seed steps and ONE `Closure::reach_from` the first time any
//! variant of its rule reaches it, and keeps the seed set ([`MultiState`]) for
//! explain.
//!
//! **Budget.** In evaluation every emitted binding counts 1 and every builtin memo,
//! closure memo, unbound-reaches and hoisted row counts too; past the cap
//! [`Budget::tripped`] is set and the run returns `Ok` early, DA.14b turning it into
//! `RulesError::Budget`. Explain counts nothing. Single-threaded; no map is iterated.

use std::ops::RangeInclusive;

use glia_activation::profile::DomainTables;
use glia_core::NodeId;

use crate::analysis::{CatSet, ConstVal, PredId};
use crate::closure::Closure;
use crate::diag::RulesError;
use crate::ir::{BuiltinDecl, CmpOp};
use crate::plan::{Bind, Check, KeyPart, Src, Step};
use crate::relation::{Cursor, EDB_RULE, Relation, RowMeta, View, Which};
use crate::source::{Builtin, FactSource, NO_ORDINAL};
use crate::value::{Handle, Lookup, Tier, ValueRef, ValueTable};

/// An unbound variable, or a free builtin argument in a memo key. No value gets this
/// handle (value.rs stops interning one short of it).
const FREE: Handle = Handle::MAX;

/// The provenance of a row no rule derived: builtin, `reaches` and filter rows.
const LOADED: RowMeta = RowMeta { height: 0, rule: EDB_RULE };

/// An scc relation's columns in identity order, and swapped (`rep` first).
const ID2: [u8; 2] = [0, 1];
const SWAP: [u8; 2] = [1, 0];

/// Builtin rows per (catalog index, argument handles with [`FREE`] for unbound).
pub(crate) type Memo = Lookup<(usize, Vec<Handle>), Vec<(Vec<Handle>, Tier)>>;

/// What one evaluation lends the interpreter.
pub(crate) struct Ctx<'e, 's, S: FactSource + ?Sized> {
    pub(crate) src: &'s S,
    pub(crate) values: &'e mut ValueTable,
    /// By PredId: the EDB, lower strata and the stratum running (an id with no
    /// relation, such as a builtin's, reads as empty).
    pub(crate) rels: &'e [Relation],
    /// One `(x, rep)` relation per scc category set, loaded from `Closure::scc_rows`;
    /// a `[1, 0]` index, when built, serves calls with only `rep` bound.
    pub(crate) scc: &'e [(CatSet, Relation)],
    pub(crate) closure: &'e mut Closure<'s, S>,
    /// By catalog index (`source::declare`'s `impls`): `None` for reaches / scc.
    pub(crate) builtins: &'e [Option<&'s dyn Builtin>],
    /// By catalog index (`Catalog::builtins`): a failure's `name/arity`, `tiered`.
    pub(crate) decls: &'e [BuiltinDecl],
    pub(crate) memo: &'e mut Memo,
    /// By ReachMulti slot (every variant of a rule shares its slot's state).
    pub(crate) multi: &'e mut Vec<MultiState>,
    pub(crate) budget: &'e mut Budget,
}

/// The raw-row budget: rows counted so far against `cap`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Budget {
    pub(crate) cap: usize,
    pub(crate) used: usize,
    /// The count when it first passed `cap`.
    pub(crate) tripped: Option<usize>,
}

impl Budget {
    pub(crate) fn new(cap: usize) -> Self {
        Budget { cap, used: 0, tripped: None }
    }

    /// Count `n` more rows; true once the count has passed the cap.
    pub(crate) fn charge(&mut self, n: usize) -> bool {
        self.used = self.used.saturating_add(n);
        if self.used > self.cap && self.tripped.is_none() {
            self.tripped = Some(self.used);
        }
        self.tripped.is_some()
    }

    /// Rows left before the cap.
    pub(crate) fn remaining(&self) -> usize {
        self.cap.saturating_sub(self.used)
    }
}

/// How a body runs. `Eval` at level L, in that level's first round or a later one;
/// `Explain` at level L over rows below `max_height`, with `bound` (variable, value)
/// set before the body (the head's, for a row).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Mode<'m> {
    Eval { level: Tier, first_round: bool },
    Explain { level: Tier, max_height: u32, bound: &'m [(u16, Handle)] },
}

/// One complete binding.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Binding<'b> {
    /// By variable; [`FREE`] past what the body binds.
    pub(crate) vars: &'b [Handle],
    pub(crate) level: Tier,
    pub(crate) height: u32,
    /// Explain only: one per row-reading step, in step order.
    pub(crate) lits: &'b [LitRow],
}

/// The row one step read for a binding (explain).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LitRow {
    /// The step's index in the steps run.
    pub(crate) step: usize,
    /// A Scan / Lookup's predicate; `None` for a builtin, reaches or hoisted row.
    pub(crate) pred: Option<PredId>,
    /// In column order: a relation's or builtin's full row, `[from, to]` for
    /// reaches, `[reached]` for a hoisted reach.
    pub(crate) row: Vec<Handle>,
    pub(crate) tier: Tier,
    pub(crate) meta: RowMeta,
    /// An EDB edge row's witness ordinal; [`NO_ORDINAL`] otherwise.
    pub(crate) ordinal: u32,
}

/// A hoisted reach's kept state: its distinct seeds with their best level, what the
/// one walk reached partitioned by level, and the highest seed height (an upper
/// bound on any reached row's, since a row does not say which seed reached it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MultiState {
    pub(crate) seeds: Vec<(NodeId, Tier)>,
    pub(crate) rows: [Vec<NodeId>; 3],
    pub(crate) height: u32,
    pub(crate) done: bool,
}

/// Run `steps` in `mode`, calling `emit` with every complete binding. Returns early,
/// `Ok`, once the budget trips (see [`Budget::tripped`]); a builtin's error and any
/// error `emit` returns stop the run.
pub(crate) fn run_body<S: FactSource + ?Sized>(
    ctx: &mut Ctx<'_, '_, S>,
    steps: &[Step],
    mode: Mode<'_>,
    emit: &mut dyn FnMut(&Binding<'_>) -> Result<(), RulesError>,
) -> Result<(), RulesError> {
    let (level, first, max_height, bound) = match mode {
        Mode::Eval { level, first_round } => (level, first_round, None, &[][..]),
        Mode::Explain { level, max_height, bound } => (level, false, Some(max_height), bound),
    };
    let explain = max_height.is_some();
    let vis = Vis { level: level.level(), first, max_height, explain, count: !explain };
    walk(ctx, steps, vis, bound, emit)
}

/// Which rows a run reads.
#[derive(Clone, Copy, Debug)]
struct Vis {
    level: u8,
    first: bool,
    max_height: Option<u32>,
    explain: bool,
    /// Charge the budget (evaluation).
    count: bool,
}

impl Vis {
    /// The levels and batches `src` reads (module doc).
    fn span(self, src: Src) -> (RangeInclusive<u8>, Which) {
        let (l, upto) = (self.level, RangeInclusive::new(0, self.level));
        if self.explain {
            return (upto, Which::Both);
        }
        match (src, self.first) {
            (Src::All, _) => (upto, Which::Both),
            (Src::Delta, true) => (l..=l, Which::Both),
            (Src::Delta, false) => (l..=l, Which::Recent),
            (Src::Old, true) => {
                (l.checked_sub(1).map_or(RangeInclusive::new(1, 0), |b| 0..=b), Which::Both)
            }
            (Src::Old, false) => (upto, Which::Stable),
        }
    }

    fn view(self, src: Src) -> View {
        let (levels, which) = self.span(src);
        View { levels, which, max_height: self.max_height }
    }

    /// A complete-input row (builtin, reaches, hoisted) at `tier` is visible.
    fn admits(self, src: Src, tier: Tier) -> bool {
        let (levels, which) = self.span(src);
        which != Which::Recent && levels.contains(&tier.level())
    }

    /// Every level, no height bound: what a negation and a hoisted seed read.
    fn complete(count: bool) -> Vis {
        let level = Tier::Heuristic.level();
        Vis { level, first: false, max_height: None, explain: false, count }
    }
}

/// One matching row of a step: `vals[at..at + len]`.
#[derive(Clone, Copy, Debug)]
struct Alt {
    at: usize,
    len: usize,
    tier: Tier,
    meta: RowMeta,
    ordinal: u32,
}

/// What a check compares a column with: a value, or another column of the same row
/// (a variable the step binds twice).
#[derive(Clone, Copy, Debug)]
enum Want {
    Val(Handle),
    Col(usize),
}

/// The alternatives of every open frame, as stacks.
#[derive(Default)]
struct Alts {
    alts: Vec<Alt>,
    vals: Vec<Handle>,
}

impl Alts {
    /// A filter step that holds: one empty alternative.
    fn unit(&mut self) {
        self.alts.push(Alt {
            at: self.vals.len(),
            len: 0,
            tier: Tier::Fact,
            meta: LOADED,
            ordinal: NO_ORDINAL,
        });
    }

    fn push(&mut self, row: &[Handle], wants: &[(usize, Want)], tier: Tier, meta: RowMeta) {
        let at = self.vals.len();
        self.vals.extend_from_slice(row);
        self.keep(at, wants, (tier, meta, NO_ORDINAL), None);
    }

    /// A permuted relation row, put back in column order. With `explain`, a row read
    /// off a non-identity index takes its witness ordinal from `Relation::contains`.
    fn relation_row(
        &mut self,
        prow: &[Handle],
        perm: &[u8],
        wants: &[(usize, Want)],
        seen: (Tier, RowMeta, u32),
        explain: Option<&Relation>,
    ) {
        let at = self.vals.len();
        self.vals.resize(at + perm.len(), FREE);
        for (&c, &h) in perm.iter().zip(prow) {
            if let Some(slot) = self.vals.get_mut(at + usize::from(c)) {
                *slot = h;
            }
        }
        self.keep(at, wants, seen, explain);
    }

    /// Keep `vals[at..]` as an alternative when it meets `wants`.
    fn keep(
        &mut self,
        at: usize,
        wants: &[(usize, Want)],
        seen: (Tier, RowMeta, u32),
        explain: Option<&Relation>,
    ) {
        let (tier, meta, ordinal) = seen;
        let row = &self.vals[at..];
        let fits = wants.iter().all(|&(c, w)| {
            let want = match w {
                Want::Val(h) => Some(h),
                Want::Col(o) => row.get(o).copied(),
            };
            want.is_some() && row.get(c).copied() == want
        });
        if !fits {
            self.vals.truncate(at);
            return;
        }
        let ordinal = match explain {
            Some(rel) if ordinal == NO_ORDINAL => {
                rel.contains(row).map_or(NO_ORDINAL, |(_, _, o)| o)
            }
            _ => ordinal,
        };
        self.alts.push(Alt { at, len: self.vals.len() - at, tier, meta, ordinal });
    }
}

/// One open step: its alternatives `from..end`, the next to take, and the binding's
/// level, height and explain rows before it.
struct Frame {
    from: usize,
    next: usize,
    end: usize,
    vals: usize,
    level: Tier,
    height: u32,
    lits: usize,
}

/// Per-run caches, by step index: a Lookup's cursor (reused across bindings, so keys
/// fed in order gallop) and an unbound reaches' rows.
struct Run<'e> {
    vis: Vis,
    cursors: Vec<Option<Cursor<'e>>>,
    all: Vec<Option<Vec<(Handle, Handle, Tier)>>>,
}

/// The interpreter loop: generate a step's alternatives, take the next one, emit at
/// the end, backtrack when a frame is exhausted.
fn walk<'e, S: FactSource + ?Sized>(
    ctx: &mut Ctx<'e, '_, S>,
    steps: &[Step],
    vis: Vis,
    bound: &[(u16, Handle)],
    emit: &mut dyn FnMut(&Binding<'_>) -> Result<(), RulesError>,
) -> Result<(), RulesError> {
    if vis.count && ctx.budget.tripped.is_some() {
        return Ok(());
    }
    let mut span = bound.iter().map(|(v, _)| usize::from(*v) + 1).max().unwrap_or(0);
    for s in steps {
        each_bind(s, |_, v| span = span.max(usize::from(v) + 1));
    }
    let mut vars = vec![FREE; span];
    for &(v, h) in bound {
        set(&mut vars, v, h);
    }
    let mut run = Run {
        vis,
        cursors: steps.iter().map(|_| None).collect(),
        all: steps.iter().map(|_| None).collect(),
    };
    let (mut out, mut frames, mut lits) =
        (Alts::default(), Vec::<Frame>::with_capacity(steps.len()), Vec::new());
    let (mut level, mut height) = (Tier::Fact, 0u32);
    loop {
        let k = frames.len();
        match steps.get(k) {
            Some(step) => {
                let (from, vals) = (out.alts.len(), out.vals.len());
                if !expand(ctx, &mut run, k, step, &vars, &mut out)? {
                    return Ok(());
                }
                frames.push(Frame {
                    from,
                    next: from,
                    end: out.alts.len(),
                    vals,
                    level,
                    height,
                    lits: lits.len(),
                });
            }
            None => {
                if vis.count && ctx.budget.charge(1) {
                    return Ok(());
                }
                emit(&Binding { vars: &vars, level, height, lits: &lits })?;
            }
        }
        loop {
            let Some(k) = frames.len().checked_sub(1) else { return Ok(()) };
            let f = &mut frames[k];
            if f.next < f.end {
                let a = out.alts[f.next];
                f.next += 1;
                level = f.level.weakest(a.tier);
                height = f.height.max(a.meta.height);
                let row = &out.vals[a.at..a.at + a.len];
                each_bind(&steps[k], |c, v| set(&mut vars, v, row.get(c).copied().unwrap_or(FREE)));
                if vis.explain {
                    lits.truncate(f.lits);
                    lits.extend(lit_row(k, &steps[k], row, &a));
                }
                break;
            }
            out.alts.truncate(f.from);
            out.vals.truncate(f.vals);
            each_bind(&steps[k], |_, v| set(&mut vars, v, FREE));
            frames.pop();
        }
    }
}

/// The variables a step binds, with the row column each comes from.
fn each_bind(step: &Step, mut f: impl FnMut(usize, u16)) {
    match step {
        Step::Scan { binds, .. }
        | Step::Lookup { binds, .. }
        | Step::Call { binds, .. }
        | Step::Reach { binds, .. } => {
            for &(c, v) in binds {
                f(usize::from(c), v);
            }
        }
        Step::Assign { var, .. } | Step::ReachMulti { bind: var, .. } => f(0, *var),
        Step::Neg { .. } | Step::Cmp { .. } | Step::NegCall { .. } | Step::NegReach { .. } => {}
    }
}

fn set(vars: &mut [Handle], v: u16, h: Handle) {
    if let Some(slot) = vars.get_mut(usize::from(v)) {
        *slot = h;
    }
}

/// The explain row of a row-reading step.
fn lit_row(k: usize, step: &Step, row: &[Handle], a: &Alt) -> Option<LitRow> {
    let pred = match step {
        Step::Scan { pred, .. } | Step::Lookup { pred, .. } => Some(*pred),
        Step::Call { .. } | Step::Reach { .. } | Step::ReachMulti { .. } => None,
        _ => return None,
    };
    Some(LitRow {
        step: k,
        pred,
        row: row.to_vec(),
        tier: a.tier,
        meta: a.meta,
        ordinal: a.ordinal,
    })
}

/// Step `k`'s alternatives under `vars`. `Ok(false)`: the budget tripped, stop.
fn expand<'e, S: FactSource + ?Sized>(
    ctx: &mut Ctx<'e, '_, S>,
    run: &mut Run<'e>,
    k: usize,
    step: &Step,
    vars: &[Handle],
    out: &mut Alts,
) -> Result<bool, RulesError> {
    let (vis, rels): (Vis, &'e [Relation]) = (run.vis, ctx.rels);
    match step {
        Step::Scan { pred, index, perm, binds, checks, src } => {
            let (Some(rel), Some(wants)) =
                (rels.get(*pred as usize), wants(ctx, checks, binds, vars))
            else {
                return Ok(true);
            };
            let ex = vis.explain.then_some(rel);
            rel.scan(*index, vis.view(*src), &mut |r, t, m, o| {
                out.relation_row(r, perm, &wants, (t, m, o), ex)
            });
        }
        Step::Lookup { pred, index, perm, key, binds, checks, src } => {
            let (Some(rel), Some(key), Some(wants)) =
                (rels.get(*pred as usize), parts(ctx, key, vars), wants(ctx, checks, binds, vars))
            else {
                return Ok(true);
            };
            let ex = vis.explain.then_some(rel);
            let Some(slot) = run.cursors.get_mut(k) else { return Ok(true) };
            let cursor = slot.get_or_insert_with(|| rel.cursor(*index, vis.view(*src)));
            cursor.seek(&key, &mut |r, t, m, o| out.relation_row(r, perm, &wants, (t, m, o), ex));
        }
        Step::Neg { pred, index, key, .. } => {
            let Some(key) = parts(ctx, key, vars) else { return Ok(true) };
            if !rels.get(*pred as usize).is_some_and(|r| r.exists_prefix(*index, &key)) {
                out.unit();
            }
        }
        Step::Cmp { op, l, r } => {
            let (Some(a), Some(b)) = (resolve(ctx, l, vars), resolve(ctx, r, vars)) else {
                return Ok(true);
            };
            if compare(ctx, *op, a, b) {
                out.unit();
            }
        }
        Step::Assign { from, .. } => {
            if let Some(h) = resolve(ctx, from, vars) {
                out.push(&[h], &[], Tier::Fact, LOADED);
            }
        }
        Step::Call { builtin, cats, args, binds, checks, src } => {
            let Some(wants) = wants(ctx, checks, binds, vars) else { return Ok(true) };
            return call(ctx, (*builtin, cats.as_ref(), args), wants, (vis, *src), vars, out);
        }
        Step::NegCall { builtin, cats, args } => {
            let mut probe = Alts::default();
            let read = (Vis::complete(vis.count), Src::All);
            if !call(ctx, (*builtin, cats.as_ref(), args), Vec::new(), read, vars, &mut probe)? {
                return Ok(false);
            }
            if probe.alts.is_empty() {
                out.unit();
            }
        }
        Step::Reach { cats, depth, from, to, binds, checks, src } => {
            let (Some(wants), Some(from), Some(to)) = (
                wants(ctx, checks, binds, vars),
                opt(ctx, from.as_ref(), vars),
                opt(ctx, to.as_ref(), vars),
            ) else {
                return Ok(true);
            };
            return reach(
                ctx,
                run.all.get_mut(k),
                (cats, *depth),
                (from, to),
                &wants,
                (vis, *src),
                out,
            );
        }
        Step::NegReach { cats, depth, from, to } => {
            let (Some(from), Some(to)) =
                (opt(ctx, from.as_ref(), vars), opt(ctx, to.as_ref(), vars))
            else {
                return Ok(true);
            };
            let mut probe = Alts::default();
            let read = (Vis::complete(vis.count), Src::All);
            if !reach(ctx, run.all.get_mut(k), (cats, *depth), (from, to), &[], read, &mut probe)? {
                return Ok(false);
            }
            if probe.alts.is_empty() {
                out.unit();
            }
        }
        Step::ReachMulti { slot, cats, depth, seed, seed_var, backward, src, .. } => {
            let done = ctx.multi.get(*slot).is_some_and(|m| m.done);
            if !done
                && !fill_multi(ctx, *slot, (cats, *depth), seed, *seed_var, *backward, vis.count)?
            {
                return Ok(false);
            }
            let Some(m) = ctx.multi.get(*slot) else { return Ok(true) };
            let meta = RowMeta { height: m.height, rule: EDB_RULE };
            for (tier, ids) in Tier::ALL.iter().zip(&m.rows) {
                if vis.admits(*src, *tier) {
                    for &id in ids {
                        out.push(&[ctx.values.intern(ValueRef::Node(id))], &[], *tier, meta);
                    }
                }
            }
        }
    }
    Ok(true)
}

/// A part's value: a bound variable's handle (`None` when unbound) or a constant,
/// interned. A category-set constant reads as its text, names sorted and joined by
/// `|` (`*` for every category), the form a domain builtin's column would take.
fn resolve<S: FactSource + ?Sized>(
    ctx: &mut Ctx<'_, '_, S>,
    part: &KeyPart,
    vars: &[Handle],
) -> Option<Handle> {
    let c = match part {
        KeyPart::Var(v) => return vars.get(usize::from(*v)).copied().filter(|h| *h != FREE),
        KeyPart::Const(c) => c,
    };
    Some(match c {
        ConstVal::Str(s) => ctx.values.intern(ValueRef::Str(s)),
        ConstVal::Int(i) => ctx.values.intern(ValueRef::Int(*i)),
        ConstVal::CatSet(set) => ctx.values.intern(ValueRef::Str(&set_text(set, ctx.src.tables()))),
    })
}

fn set_text(set: &CatSet, tables: &DomainTables) -> String {
    let ids = match set {
        CatSet::All => return "*".to_string(),
        CatSet::Carry => tables.carry_edges,
        CatSet::Of(ids) => ids.as_slice(),
    };
    let reg = &tables.registries;
    let mut names: Vec<String> = ids
        .iter()
        .map(|c| reg.category_name(*c).map_or_else(|| format!("#{}", c.0), str::to_string))
        .collect();
    names.sort();
    names.join("|")
}

/// Every part resolved; `None` when one is an unbound variable.
fn parts<S: FactSource + ?Sized>(
    ctx: &mut Ctx<'_, '_, S>,
    key: &[KeyPart],
    vars: &[Handle],
) -> Option<Vec<Handle>> {
    key.iter().map(|p| resolve(ctx, p, vars)).collect()
}

/// An optional end or argument: `Some(None)` free, `None` unresolvable.
fn opt<S: FactSource + ?Sized>(
    ctx: &mut Ctx<'_, '_, S>,
    part: Option<&KeyPart>,
    vars: &[Handle],
) -> Option<Option<Handle>> {
    match part {
        None => Some(None),
        Some(p) => resolve(ctx, p, vars).map(Some),
    }
}

/// A step's checks as wants: a variable the step itself binds compares with its
/// bind column, anything else with its value.
fn wants<S: FactSource + ?Sized>(
    ctx: &mut Ctx<'_, '_, S>,
    checks: &[Check],
    binds: &[Bind],
    vars: &[Handle],
) -> Option<Vec<(usize, Want)>> {
    checks
        .iter()
        .map(|(c, part)| {
            let own = match part {
                KeyPart::Var(v) => {
                    binds.iter().find(|(_, b)| b == v).map(|(bc, _)| Want::Col(usize::from(*bc)))
                }
                KeyPart::Const(_) => None,
            };
            let w = match own {
                Some(w) => w,
                None => Want::Val(resolve(ctx, part, vars)?),
            };
            Some((usize::from(*c), w))
        })
        .collect()
}

/// `=` / `!=` compare handles; the `<` family compares ints numerically, strings by
/// bytes and nodes by rendered key, and is false across types.
fn compare<S: FactSource + ?Sized>(
    ctx: &mut Ctx<'_, '_, S>,
    op: CmpOp,
    a: Handle,
    b: Handle,
) -> bool {
    let ord = match op {
        CmpOp::Eq => return a == b,
        CmpOp::Ne => return a != b,
        CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => {
            match (ctx.values.get(a), ctx.values.get(b)) {
                (ValueRef::Node(_), ValueRef::Node(_)) => {
                    let src = ctx.src;
                    Some(ctx.values.cmp_rendered(a, b, &|id| src.render_node(id)))
                }
                _ => ctx.values.cmp_values(a, b),
            }
        }
    };
    ord.is_some_and(|o| match op {
        CmpOp::Lt => o.is_lt(),
        CmpOp::Le => o.is_le(),
        CmpOp::Gt => o.is_gt(),
        CmpOp::Ge => o.is_ge(),
        CmpOp::Eq | CmpOp::Ne => false,
    })
}

fn node_of(values: &ValueTable, h: Handle) -> Option<NodeId> {
    match values.get(h) {
        ValueRef::Node(n) => Some(n),
        ValueRef::Str(_) | ValueRef::Int(_) => None,
    }
}

/// A builtin call's rows: a domain / string builtin through the memo, or an `scc`
/// call's rows of its category set's relation. Each bound argument must equal its
/// column. The second argument is the call (catalog index, scc category set,
/// arguments), the fourth the run's visibility and the literal's source.
fn call<S: FactSource + ?Sized>(
    ctx: &mut Ctx<'_, '_, S>,
    (b, cats, args): (usize, Option<&CatSet>, &[Option<KeyPart>]),
    mut wants: Vec<(usize, Want)>,
    (vis, src): (Vis, Src),
    vars: &[Handle],
    out: &mut Alts,
) -> Result<bool, RulesError> {
    let imp = ctx.builtins.get(b).copied().flatten();
    // An scc call's third column is its category set, carried by `cats`.
    let n = if imp.is_some() { args.len() } else { args.len().min(2) };
    let mut key = Vec::with_capacity(n);
    for (c, a) in args[..n].iter().enumerate() {
        let Some(h) = opt(ctx, a.as_ref(), vars) else { return Ok(true) };
        key.push(h.unwrap_or(FREE));
        if let Some(h) = h {
            wants.push((c, Want::Val(h)));
        }
    }
    let Some(imp) = imp else {
        let rel = cats.and_then(|c| ctx.scc.iter().find(|(s, _)| s == c)).map(|(_, r)| r);
        if let (Some(rel), [x, rep]) = (rel, key.as_slice()) {
            scc(rel, *x, *rep, &wants, vis.view(src), out);
        }
        return Ok(true);
    };
    let key = (b, key);
    if !ctx.memo.contains_key(&key) && !fill_memo(ctx, &key, imp, vis.count)? {
        return Ok(false);
    }
    for (row, tier) in ctx.memo.get(&key).map_or(&[][..], Vec::as_slice) {
        if vis.admits(src, *tier) {
            out.push(row, &wants, *tier, LOADED);
        }
    }
    Ok(true)
}

/// Call builtin `key.0` once and memoise its interned rows; `Ok(false)` when they
/// trip the budget.
fn fill_memo<S: FactSource + ?Sized>(
    ctx: &mut Ctx<'_, '_, S>,
    key: &(usize, Vec<Handle>),
    imp: &dyn Builtin,
    count: bool,
) -> Result<bool, RulesError> {
    let mut found = Vec::new();
    let called = {
        let values = &*ctx.values;
        let args: Vec<Option<ValueRef<'_>>> =
            key.1.iter().map(|&h| (h != FREE).then(|| values.get(h))).collect();
        imp.call(&args, &mut found)
    };
    let decl = ctx.decls.get(key.0);
    if let Err(message) = called {
        let name = decl.map_or_else(|| imp.decl().signature(), BuiltinDecl::signature);
        return Err(RulesError::Builtin { name, message });
    }
    let tiered = decl.is_none_or(|d| d.tiered);
    let rows: Vec<(Vec<Handle>, Tier)> = found
        .iter()
        .filter(|r| r.values.len() == key.1.len())
        .map(|r| {
            let row = r.values.iter().map(|v| ctx.values.intern(v.as_ref())).collect();
            (row, if tiered { r.tier } else { Tier::Fact })
        })
        .collect();
    let n = rows.len();
    ctx.memo.insert(key.clone(), rows);
    Ok(!(count && ctx.budget.charge(n)))
}

/// The `(x, rep)` rows of an scc relation with `x` / `rep` bound or [`FREE`]: by
/// prefix on the identity index, by `rep` on a `[1, 0]` index when one is built, else
/// a scan the wants filter.
fn scc(
    rel: &Relation,
    x: Handle,
    rep: Handle,
    wants: &[(usize, Want)],
    view: View,
    out: &mut Alts,
) {
    let (index, perm, key): (usize, &[u8], Vec<Handle>) =
        match (x != FREE, rep != FREE, rel.index_of(&SWAP)) {
            (true, true, _) => (0, &ID2, vec![x, rep]),
            (true, false, _) => (0, &ID2, vec![x]),
            (false, true, Some(i)) => (i, &SWAP, vec![rep]),
            _ => (0, &ID2, Vec::new()),
        };
    let f = &mut |r: &[Handle], t, m, o| out.relation_row(r, perm, wants, (t, m, o), None);
    if key.is_empty() {
        rel.scan(index, view, f);
    } else {
        rel.cursor(index, view).seek(&key, f);
    }
}

/// `reaches(from, to)` rows (`None` a free end): per source through the closure's
/// memo, backward from a bound target alone, or every pair once per run when both
/// are free (`cache` is the step's slot for those rows). A bound end that is no node
/// reaches nothing. `Ok(false)`: the budget tripped.
fn reach<S: FactSource + ?Sized>(
    ctx: &mut Ctx<'_, '_, S>,
    cache: Option<&mut Option<Vec<(Handle, Handle, Tier)>>>,
    (cats, depth): (&CatSet, Option<u32>),
    (from, to): (Option<Handle>, Option<Handle>),
    wants: &[(usize, Want)],
    (vis, src): (Vis, Src),
    out: &mut Alts,
) -> Result<bool, RulesError> {
    let fid = from.map(|h| node_of(ctx.values, h));
    let tid = to.map(|h| node_of(ctx.values, h));
    if matches!(fid, Some(None)) || matches!(tid, Some(None)) {
        return Ok(true);
    }
    let before = ctx.closure.stats().memo_rows();
    match (from.zip(fid.flatten()), to.zip(tid.flatten())) {
        (Some((f, fid)), to) => {
            for &(y, t) in ctx.closure.reach(cats, fid, false, depth) {
                if vis.admits(src, t) && to.is_none_or(|(_, w)| w == y) {
                    let y = to.map_or_else(|| ctx.values.intern(ValueRef::Node(y)), |(h, _)| h);
                    out.push(&[f, y], wants, t, LOADED);
                }
            }
        }
        (None, Some((th, tid))) => {
            for &(x, t) in ctx.closure.reach(cats, tid, true, depth) {
                if vis.admits(src, t) {
                    out.push(&[ctx.values.intern(ValueRef::Node(x)), th], wants, t, LOADED);
                }
            }
        }
        (None, None) => {
            let Some(cache) = cache else { return Ok(true) };
            if cache.is_none() {
                let limit = if vis.count { ctx.budget.remaining() } else { usize::MAX };
                let Some(rows) = ctx.closure.reach_all(cats, depth, limit) else {
                    // Only a budgeted run passes a limit the rows can exceed.
                    if vis.count {
                        ctx.budget.charge(limit.saturating_add(1));
                    }
                    return Ok(false);
                };
                let node = |values: &mut ValueTable, id| values.intern(ValueRef::Node(id));
                let rows: Vec<_> = rows
                    .into_iter()
                    .map(|(x, y, t)| (node(ctx.values, x), node(ctx.values, y), t))
                    .collect();
                if vis.count && ctx.budget.charge(rows.len()) {
                    return Ok(false);
                }
                *cache = Some(rows);
            }
            for &(x, y, t) in cache.iter().flatten() {
                if vis.admits(src, t) {
                    out.push(&[x, y], wants, t, LOADED);
                }
            }
        }
    }
    let grown = ctx.closure.stats().memo_rows().saturating_sub(before);
    Ok(!(vis.count && ctx.budget.charge(grown)))
}

/// Run a hoisted reach's seed steps over complete input, keep each distinct node
/// seed with its best level (and its lowest height there), walk once from all of
/// them and store the result in `slot`. `Ok(false)`: the budget tripped.
fn fill_multi<S: FactSource + ?Sized>(
    ctx: &mut Ctx<'_, '_, S>,
    slot: usize,
    (cats, depth): (&CatSet, Option<u32>),
    seed: &[Step],
    seed_var: u16,
    backward: bool,
    count: bool,
) -> Result<bool, RulesError> {
    debug_assert!(seed.iter().all(|s| complete_input(s, ctx.rels)), "a seed reads complete input");
    let vis = Vis::complete(count);
    let mut raw: Vec<(Handle, Tier, u32)> = Vec::new();
    let take = usize::from(seed_var);
    walk(ctx, seed, vis, &[], &mut |b| {
        raw.extend(b.vars.get(take).filter(|h| **h != FREE).map(|h| (*h, b.level, b.height)));
        Ok(())
    })?;
    if count && ctx.budget.tripped.is_some() {
        return Ok(false);
    }
    let mut at: Lookup<Handle, usize> = Lookup::new();
    let mut seeds: Vec<(NodeId, Tier, u32)> = Vec::new();
    for (h, t, height) in raw {
        let Some(id) = node_of(ctx.values, h) else { continue };
        match at.get(&h) {
            Some(&i) => {
                let s = &mut seeds[i];
                if (t, height) < (s.1, s.2) {
                    (s.1, s.2) = (t, height);
                }
            }
            None => {
                at.insert(h, seeds.len());
                seeds.push((id, t, height));
            }
        }
    }
    let starts: Vec<(NodeId, Tier)> = seeds.iter().map(|s| (s.0, s.1)).collect();
    let reached = ctx.closure.reach_from(cats, &starts, backward, depth);
    let height = seeds.iter().map(|s| s.2).max().unwrap_or(0);
    let mut state = MultiState { seeds: starts, rows: Default::default(), height, done: true };
    for (id, t) in &reached {
        if let Some(part) = state.rows.get_mut(usize::from(t.level())) {
            part.push(*id);
        }
    }
    let n = state.seeds.len() + reached.len();
    if ctx.multi.len() <= slot {
        ctx.multi.resize_with(slot + 1, MultiState::default);
    }
    ctx.multi[slot] = state;
    Ok(!(count && ctx.budget.charge(n)))
}

/// A hoisted seed step reads complete input: everything `Src::All`, and no relation
/// with a recent run (a lower stratum or the EDB, never the stratum running).
fn complete_input(step: &Step, rels: &[Relation]) -> bool {
    match step {
        Step::Scan { pred, src, .. } | Step::Lookup { pred, src, .. } => {
            *src == Src::All && rels.get(*pred as usize).is_none_or(|r| r.recent_len() == 0)
        }
        Step::Call { src, .. } | Step::Reach { src, .. } | Step::ReachMulti { src, .. } => {
            *src == Src::All
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use glia_core::Confidence;

    use super::*;
    use crate::analysis::{Analysis, PredKind, analyse};
    use crate::ir::{Atom, BindReq, BuiltinKind, ColType, HeadArg, Literal, Program, Rule, Term};
    use crate::plan::{StratumPlan, Variant, plan_bound, plan_stratum, resolve_indexes};
    use crate::source::{
        BuiltinRow, Declared, EdbRows, MemSource, Restrict, declare, edge_tiers, load_edb,
    };
    use crate::testkit::{GraphSpec, category_id, chain, lcg_graph, node_id};
    use crate::value::Value;

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

    /// `n` numbered testkit nodes (`n<i:05>`, `index` = i, salt 0) and no edges.
    fn nodes(n: u32) -> MemSource {
        lcg_graph(&GraphSpec {
            nodes: n,
            edges: 0,
            categories: vec![],
            tier_mix: [100, 0, 0],
            seed: 1,
            salt: 0,
        })
    }

    fn edge(g: &mut MemSource, from: u32, to: u32, cat: &str, tier: Tier) {
        g.edge(node_id(0, from), node_id(0, to), category_id(cat), Confidence::Strong, tier);
    }

    fn row(vals: &[&str], tier: Tier) -> (Vec<String>, Tier) {
        (vals.iter().map(|x| x.to_string()).collect(), tier)
    }

    fn calls() -> CatSet {
        CatSet::Of(vec![category_id("CALLS")])
    }

    /// One analysed program over a source, every EDB relation loaded.
    struct World<'s> {
        src: &'s MemSource,
        an: Analysis,
        decl: Declared<'s>,
        values: ValueTable,
        rels: Vec<Relation>,
        scc: Vec<(CatSet, Relation)>,
        closure: Closure<'s, MemSource>,
        memo: Memo,
        multi: Vec<MultiState>,
        budget: Budget,
    }

    impl<'s> World<'s> {
        fn new(src: &'s MemSource, tiers: &'s [Tier], rules: Vec<Rule>) -> Self {
            let decl = declare(src);
            let mut p = Program::new();
            p.add_source("t");
            p.rules = rules;
            let an = analyse(&p, &decl.catalog)
                .unwrap_or_else(|e| panic!("expected an analysis, got: {e}"));
            let mut values = ValueTable::new();
            let rels = an
                .preds
                .iter()
                .map(|p| {
                    let mut r = Relation::new(p.arity, &[]);
                    if let PredKind::Edb(e) = p.kind {
                        r.load(load_edb(src, e, &Restrict::default(), tiers, &mut values), None);
                    }
                    r
                })
                .collect();
            let closure = Closure::new(src, tiers);
            let (memo, multi, budget) = (Memo::new(), Vec::new(), Budget::new(usize::MAX));
            World { src, an, decl, values, rels, scc: Vec::new(), closure, memo, multi, budget }
        }

        fn pid(&self, name: &str) -> PredId {
            let p = self.an.preds.iter().position(|p| p.name == name);
            p.map(|p| p as PredId).unwrap_or_else(|| panic!("no predicate {name}"))
        }

        /// The plan of `head`'s stratum, its indexes built and resolved.
        fn plan(&mut self, head: &str) -> StratumPlan {
            let stratum = self.an.preds[self.pid(head) as usize].stratum;
            let mut plan = plan_stratum(&self.an, &self.decl.catalog, stratum, &|_| None);
            let map = self.ensure(&plan.indexes);
            plan.resolve(&map);
            plan
        }

        fn ensure(&mut self, indexes: &[(PredId, Vec<u8>)]) -> Vec<usize> {
            let at = |(p, perm): &(PredId, Vec<u8>), rels: &mut Vec<Relation>| {
                rels[*p as usize].ensure_index(perm)
            };
            indexes
                .iter()
                .map(|i| at(i, &mut self.rels).unwrap_or_else(|| panic!("no index {i:?}")))
                .collect()
        }

        fn ctx(&mut self) -> Ctx<'_, 's, MemSource> {
            Ctx {
                src: self.src,
                values: &mut self.values,
                rels: &self.rels,
                scc: &self.scc,
                closure: &mut self.closure,
                builtins: &self.decl.impls,
                decls: &self.decl.catalog.builtins,
                memo: &mut self.memo,
                multi: &mut self.multi,
                budget: &mut self.budget,
            }
        }

        fn render(&self, h: Handle) -> String {
            match self.values.get(h) {
                ValueRef::Node(n) => self.src.render_node(n),
                ValueRef::Str(x) => x.to_string(),
                ValueRef::Int(i) => i.to_string(),
            }
        }

        fn handle(&self, i: u32) -> Handle {
            self.values
                .find(ValueRef::Node(node_id(0, i)))
                .unwrap_or_else(|| panic!("node {i} not interned"))
        }

        /// Run `steps`; each binding's `head` rendered, with its level, sorted.
        fn run(
            &mut self,
            steps: &[Step],
            head: &[KeyPart],
            mode: Mode<'_>,
        ) -> Result<Vec<(Vec<String>, Tier)>, RulesError> {
            let mut raw: Vec<(Vec<Handle>, Tier)> = Vec::new();
            run_body(&mut self.ctx(), steps, mode, &mut |b| {
                let vals = head.iter().map(|p| match p {
                    KeyPart::Var(x) => b.vars[usize::from(*x)],
                    KeyPart::Const(_) => FREE,
                });
                raw.push((vals.collect(), b.level));
                Ok(())
            })?;
            let mut out: Vec<_> = raw
                .into_iter()
                .map(|(hs, t)| (hs.iter().map(|h| self.render(*h)).collect(), t))
                .collect();
            out.sort();
            Ok(out)
        }
    }

    fn eval(level: Tier) -> Mode<'static> {
        Mode::Eval { level, first_round: true }
    }

    fn base(plan: &StratumPlan, rule: usize) -> &Variant {
        let r =
            plan.rules.iter().find(|r| r.rule == rule).unwrap_or_else(|| panic!("no rule {rule}"));
        r.variants.iter().find(|v| v.delta.is_none()).unwrap_or_else(|| panic!("no base variant"))
    }

    fn delta(plan: &StratumPlan, rule: usize, d: usize) -> &Variant {
        let r =
            plan.rules.iter().find(|r| r.rule == rule).unwrap_or_else(|| panic!("no rule {rule}"));
        r.variants.iter().find(|v| v.delta == Some(d)).unwrap_or_else(|| panic!("no variant {d}"))
    }

    #[test]
    fn join_negation_comparison() {
        let mut g = chain(
            6,
            "CALLS",
            &[Tier::Fact, Tier::Derived, Tier::Fact, Tier::Heuristic, Tier::Fact],
        );
        edge(&mut g, 1, 0, "CALLS", Tier::Fact);
        edge(&mut g, 2, 0, "CALLS", Tier::Derived);
        let tiers = edge_tiers(&g);
        let r = rule(
            "v",
            vec![v("a"), v("b")],
            vec![
                pos("edge", vec![v("a"), v("b"), s("CALLS")]),
                neg("edge", vec![v("b"), v("a"), s("CALLS")]),
                pos("index", vec![v("a"), v("i")]),
                Literal::cmp(CmpOp::Lt, v("i"), Term::int(3)),
            ],
        );
        let mut w = World::new(&g, &tiers, vec![r]);
        let plan = w.plan("v");
        let b = base(&plan, 0);
        // 0 -> 1 and 1 -> 0 block each other; 3 -> 4 and 4 -> 5 fail i < 3.
        let all = vec![
            row(&["n00001", "n00002"], Tier::Derived),
            row(&["n00002", "n00000"], Tier::Derived),
            row(&["n00002", "n00003"], Tier::Fact),
        ];
        assert_eq!(w.run(&b.steps, &b.head, eval(Tier::Heuristic)).unwrap(), all);
        assert_eq!(w.run(&b.steps, &b.head, eval(Tier::Fact)).unwrap(), vec![all[2].clone()]);
        // Level DERIVED's first round: the edge literal's delta is its DERIVED partition.
        let d = delta(&plan, 0, 0);
        assert_eq!(w.run(&d.steps, &d.head, eval(Tier::Derived)).unwrap(), all[..2].to_vec());
        // A later round's delta is the recent run, which complete input never has.
        let later = Mode::Eval { level: Tier::Derived, first_round: false };
        assert_eq!(w.run(&d.steps, &d.head, later).unwrap(), vec![]);
        // Explain with b bound reads edge on its [1, 2, 0] index, whose rows carry no
        // ordinal: the witness comes from the identity index (2 -> 3 is edge 2).
        let (mut steps, indexes) = plan_bound(&w.an, &w.decl.catalog, 0, &[false, true]);
        assert!(indexes.iter().any(|(_, perm)| perm == &[1, 2, 0]), "{indexes:?}");
        let map = w.ensure(&indexes);
        resolve_indexes(&mut steps, &map);
        let bound = [(1u16, w.handle(3))];
        let mode = Mode::Explain { level: Tier::Heuristic, max_height: u32::MAX, bound: &bound };
        let mut lits = Vec::new();
        run_body(&mut w.ctx(), &steps, mode, &mut |b| {
            lits.push(b.lits.iter().map(|l| (l.pred, l.row.len(), l.ordinal)).collect::<Vec<_>>());
            Ok(())
        })
        .unwrap();
        let (edge, index) = (Some(w.pid("edge")), Some(w.pid("index")));
        assert_eq!(lits, vec![vec![(edge, 3, 2), (index, 2, NO_ORDINAL)]]);
    }

    #[test]
    fn negation_reads_all_levels() {
        let mut g = chain(4, "CALLS", &[Tier::Fact]);
        edge(&mut g, 1, 0, "CALLS", Tier::Heuristic);
        let tiers = edge_tiers(&g);
        let r = rule(
            "v",
            vec![v("a"), v("b")],
            vec![
                pos("edge", vec![v("a"), v("b"), s("CALLS")]),
                neg("edge", vec![v("b"), v("a"), s("CALLS")]),
            ],
        );
        let mut w = World::new(&g, &tiers, vec![r]);
        let plan = w.plan("v");
        let b = base(&plan, 0);
        let open =
            vec![row(&["n00001", "n00002"], Tier::Fact), row(&["n00002", "n00003"], Tier::Fact)];
        // At FACT the HEURISTIC 1 -> 0 is no positive row, yet it blocks (0, 1).
        assert_eq!(w.run(&b.steps, &b.head, eval(Tier::Fact)).unwrap(), open);
        assert_eq!(w.run(&b.steps, &b.head, eval(Tier::Heuristic)).unwrap(), open);
    }

    #[test]
    fn binding_level_is_the_weakest_row() {
        let mut g = nodes(6);
        edge(&mut g, 0, 1, "CALLS", Tier::Derived);
        edge(&mut g, 1, 2, "USES", Tier::Fact);
        edge(&mut g, 3, 4, "CALLS", Tier::Fact);
        edge(&mut g, 4, 5, "USES", Tier::Fact);
        let tiers = edge_tiers(&g);
        let r = rule(
            "v",
            vec![v("a"), v("c")],
            vec![
                pos("edge", vec![v("a"), v("b"), s("CALLS")]),
                pos("edge", vec![v("b"), v("c"), s("USES")]),
            ],
        );
        let mut w = World::new(&g, &tiers, vec![r]);
        let plan = w.plan("v");
        let b = base(&plan, 0);
        let fact = row(&["n00003", "n00005"], Tier::Fact);
        assert_eq!(
            w.run(&b.steps, &b.head, eval(Tier::Derived)).unwrap(),
            vec![row(&["n00000", "n00002"], Tier::Derived), fact.clone()]
        );
        assert_eq!(w.run(&b.steps, &b.head, eval(Tier::Fact)).unwrap(), vec![fact]);
    }

    /// `in_scope(Node, Str)`: the nodes of a named scope, counting its calls.
    struct InScope {
        calls: Rc<Cell<usize>>,
        cli: Vec<NodeId>,
    }

    impl Builtin for InScope {
        fn decl(&self) -> BuiltinDecl {
            BuiltinDecl {
                name: "in_scope".into(),
                cols: vec![ColType::Node, ColType::Str],
                bind: vec![BindReq::May, BindReq::Must],
                tiered: false,
                kind: BuiltinKind::Domain,
            }
        }

        fn call(
            &self,
            args: &[Option<ValueRef<'_>>],
            out: &mut Vec<BuiltinRow>,
        ) -> Result<(), String> {
            self.calls.set(self.calls.get() + 1);
            let (node, Some(ValueRef::Str(scope))) = (args[0], args[1]) else {
                return Err("bad call".into());
            };
            if scope != "cli" {
                return Err(format!("scope `{scope}` holds no node"));
            }
            for id in &self.cli {
                if node.is_none_or(|n| n == ValueRef::Node(*id)) {
                    // A HEURISTIC claim from an untiered builtin still reads FACT.
                    out.push(BuiltinRow {
                        values: vec![Value::Node(*id), Value::Str(scope.into())],
                        tier: Tier::Heuristic,
                    });
                }
            }
            Ok(())
        }
    }

    #[test]
    fn builtin_memo_and_error() {
        let mut g = nodes(4);
        edge(&mut g, 0, 2, "CALLS", Tier::Fact);
        edge(&mut g, 1, 2, "CALLS", Tier::Fact);
        let calls = Rc::new(Cell::new(0));
        g.builtin(Box::new(InScope { calls: calls.clone(), cli: vec![node_id(0, 2)] }));
        let tiers = edge_tiers(&g);
        let ok = rule(
            "v",
            vec![v("a"), v("b")],
            vec![
                pos("edge", vec![v("a"), v("b"), s("CALLS")]),
                pos("in_scope", vec![v("b"), s("cli")]),
            ],
        );
        let bad = rule(
            "w",
            vec![v("a")],
            vec![
                pos("edge", vec![v("a"), v("b"), s("CALLS")]),
                pos("in_scope", vec![v("b"), s("nowhere")]),
            ],
        );
        let mut w = World::new(&g, &tiers, vec![ok, bad]);
        let plan = w.plan("v");
        let b = base(&plan, 0);
        assert!(matches!(b.steps[0], Step::Scan { .. } | Step::Lookup { .. }), "{:?}", b.steps);
        let want =
            vec![row(&["n00000", "n00002"], Tier::Fact), row(&["n00001", "n00002"], Tier::Fact)];
        assert_eq!(w.run(&b.steps, &b.head, eval(Tier::Fact)).unwrap(), want);
        assert_eq!(calls.get(), 1, "two bindings, one call: in_scope(n00002, \"cli\")");
        assert_eq!(w.run(&b.steps, &b.head, eval(Tier::Heuristic)).unwrap(), want);
        assert_eq!(calls.get(), 1, "the memo outlives a run");
        let plan = w.plan("w");
        let b = base(&plan, 1);
        let err = w.run(&b.steps, &b.head, eval(Tier::Fact)).unwrap_err();
        let message = "scope `nowhere` holds no node".to_string();
        assert_eq!(err, RulesError::Builtin { name: "in_scope/2".into(), message });
        assert_eq!(err.to_string(), "builtin `in_scope/2` failed: scope `nowhere` holds no node");
    }

    #[test]
    fn reach_per_binding_and_multi() {
        // 0 -> 1 FACT, 1 -> 2 DERIVED, 2 -> 3 HEURISTIC, 3 -> 4 FACT, 4 -> 5 DERIVED;
        // the FUNCTION nodes are n00000 and n00004.
        let g = chain(6, "CALLS", &[Tier::Fact, Tier::Derived, Tier::Heuristic]);
        let tiers = edge_tiers(&g);
        let reaches = || pos("reaches", vec![v("a"), v("b"), s("CALLS")]);
        let function = || pos("node", vec![v("a"), s("FUNCTION")]);
        // reaches first: neither literal binds a column (a category set is no key), so
        // source order runs the walk with both ends free.
        let small = vec![
            reaches(),
            pos("index", vec![v("a"), v("i")]),
            Literal::cmp(CmpOp::Lt, v("i"), Term::int(2)),
        ];
        let rules = vec![
            rule("pair", vec![v("a"), v("b")], small),
            rule("per", vec![v("a"), v("b")], vec![function(), reaches()]),
            rule("hit", vec![v("b")], vec![function(), reaches()]),
        ];
        let mut w = World::new(&g, &tiers, rules);
        let mut oracle = Closure::new(&g, &tiers);
        let mut expect = |sources: &[u32]| {
            let mut out = Vec::new();
            for &a in sources {
                for &(y, t) in oracle.reach(&calls(), node_id(0, a), false, None) {
                    if t <= Tier::Derived {
                        out.push((vec![g.render_node(node_id(0, a)), g.render_node(y)], t));
                    }
                }
            }
            out.sort();
            out
        };
        // Both ends free (no bound column comes first): every pair, once per run.
        let plan = w.plan("pair");
        let b = base(&plan, 0);
        assert!(matches!(b.steps[0], Step::Reach { from: None, to: None, .. }), "{:?}", b.steps);
        let got = w.run(&b.steps, &b.head, eval(Tier::Derived)).unwrap();
        assert_eq!(got, expect(&[0, 1]));
        let literal = vec![
            row(&["n00000", "n00001"], Tier::Fact),
            row(&["n00000", "n00002"], Tier::Derived),
            row(&["n00001", "n00002"], Tier::Derived),
        ];
        assert_eq!(got, literal);
        // Per binding: `a` is live in the head, so no hoist.
        let plan = w.plan("per");
        let b = base(&plan, 1);
        assert!(matches!(b.steps[1], Step::Reach { from: Some(_), to: None, .. }), "{:?}", b.steps);
        let got = w.run(&b.steps, &b.head, eval(Tier::Derived)).unwrap();
        assert_eq!(got, expect(&[0, 4]));
        let literal = vec![
            row(&["n00000", "n00001"], Tier::Fact),
            row(&["n00000", "n00002"], Tier::Derived),
            row(&["n00004", "n00005"], Tier::Derived),
        ];
        assert_eq!(got, literal);
        // Hoisted: one walk from both seeds, kept across runs and variants.
        let plan = w.plan("hit");
        let b = base(&plan, 2);
        assert!(matches!(b.steps[0], Step::ReachMulti { slot: 0, .. }), "{:?}", b.steps);
        let all = w.run(&b.steps, &b.head, eval(Tier::Heuristic)).unwrap();
        let reached = vec![
            row(&["n00001"], Tier::Fact),
            row(&["n00002"], Tier::Derived),
            row(&["n00003"], Tier::Heuristic),
            row(&["n00004"], Tier::Heuristic),
            row(&["n00005"], Tier::Derived),
        ];
        assert_eq!(all, reached);
        let walks = |w: &World<'_>| {
            w.closure.stats().sets.iter().map(|s| (s.bfs, s.multi)).collect::<Vec<_>>()
        };
        let walked = walks(&w);
        // Level DERIVED's first round: the composite's delta is its DERIVED rows.
        let d = delta(&plan, 2, 1);
        let derived = vec![reached[1].clone(), reached[4].clone()];
        assert_eq!(w.run(&d.steps, &d.head, eval(Tier::Derived)).unwrap(), derived);
        assert_eq!(walks(&w), walked, "the second run reads the kept rows: no walk");
        let m = &w.multi[0];
        assert!(m.done);
        assert_eq!(m.seeds, vec![(node_id(0, 0), Tier::Fact), (node_id(0, 4), Tier::Fact)]);
        assert_eq!(m.rows.iter().map(Vec::len).collect::<Vec<_>>(), [1, 2, 2]);
    }

    #[test]
    fn budget_trips() {
        let g = nodes(100);
        let tiers = edge_tiers(&g);
        let r = rule(
            "v",
            vec![v("a"), v("b")],
            vec![pos("node", vec![v("a"), Term::wild()]), pos("node", vec![v("b"), Term::wild()])],
        );
        let mut w = World::new(&g, &tiers, vec![r]);
        w.budget = Budget::new(1000);
        let plan = w.plan("v");
        let b = base(&plan, 0);
        assert_eq!(w.run(&b.steps, &b.head, eval(Tier::Fact)).unwrap().len(), 1000);
        assert_eq!(w.budget.tripped, Some(1001));
        assert_eq!(
            w.run(&b.steps, &b.head, eval(Tier::Fact)).unwrap().len(),
            0,
            "a tripped budget emits no more"
        );
        assert_eq!(w.budget.tripped, Some(1001));
    }

    #[test]
    fn explain_mode_heights() {
        let g = nodes(8);
        let tiers = edge_tiers(&g);
        let t = rule("t", vec![v("x")], vec![pos("node", vec![v("x"), s("FUNCTION")])]);
        let u = rule("u", vec![v("x")], vec![pos("t", vec![v("x")])]);
        let mut w = World::new(&g, &tiers, vec![t, u]);
        // t's rows as evaluation would have left them: n0 at height 1, n4 at height 2.
        let (n0, n4) = (w.handle(0), w.handle(4));
        let tp = w.pid("t") as usize;
        let mut metas = vec![RowMeta { height: 1, rule: 0 }, RowMeta { height: 2, rule: 0 }];
        w.rels[tp].insert_round(Tier::Fact, &mut vec![n0, n4], &mut metas);
        let (mut steps, indexes) = plan_bound(&w.an, &w.decl.catalog, 1, &[true]);
        let map = w.ensure(&indexes);
        resolve_indexes(&mut steps, &map);
        let explain = |w: &mut World<'_>, x: Handle, max_height: u32| {
            let mut seen = Vec::new();
            let bound = [(0u16, x)];
            let mode = Mode::Explain { level: Tier::Fact, max_height, bound: &bound };
            run_body(&mut w.ctx(), &steps, mode, &mut |b| {
                seen.push((b.height, b.lits.to_vec()));
                Ok(())
            })
            .unwrap();
            seen
        };
        let lit = |x: Handle, height: u32| LitRow {
            step: 0,
            pred: Some(tp as PredId),
            row: vec![x],
            tier: Tier::Fact,
            meta: RowMeta { height, rule: 0 },
            ordinal: NO_ORDINAL,
        };
        assert_eq!(explain(&mut w, n0, 2), vec![(1, vec![lit(n0, 1)])]);
        assert_eq!(explain(&mut w, n4, 2), vec![], "a row of height 2 is not below 2");
        assert_eq!(explain(&mut w, n4, 3), vec![(2, vec![lit(n4, 2)])]);
    }

    #[test]
    fn scc_assign_and_negated_builtins() {
        // 0 -> 1 -> 2 -> 3 and 2 -> 1: one component {1, 2}, represented by n00001.
        let mut g = chain(4, "CALLS", &[Tier::Fact]);
        edge(&mut g, 2, 1, "CALLS", Tier::Fact);
        let tiers = edge_tiers(&g);
        let rules = vec![
            rule("cyc", vec![v("x"), v("r")], vec![pos("scc", vec![v("x"), v("r"), s("CALLS")])]),
            rule(
                "onrep",
                vec![v("x")],
                vec![
                    pos("index", vec![v("r"), Term::int(1)]),
                    pos("scc", vec![v("x"), v("r"), s("CALLS")]),
                ],
            ),
            rule(
                "acyc",
                vec![v("x")],
                vec![
                    pos("node", vec![v("x"), Term::wild()]),
                    neg("reaches", vec![v("x"), v("x"), s("CALLS")]),
                ],
            ),
            rule(
                "noscc",
                vec![v("x")],
                vec![
                    pos("node", vec![v("x"), Term::wild()]),
                    neg("scc", vec![v("x"), Term::wild(), s("CALLS")]),
                ],
            ),
            rule(
                "twin",
                vec![v("x"), v("y")],
                vec![
                    pos("index", vec![v("x"), v("i")]),
                    Literal::cmp(CmpOp::Eq, v("j"), v("i")),
                    pos("index", vec![v("y"), v("j")]),
                ],
            ),
        ];
        let mut w = World::new(&g, &tiers, rules);
        let mut rows = EdbRows { arity: 2, ..EdbRows::default() };
        for (x, rep, t) in w.closure.scc_rows(&calls()) {
            rows.rows
                .extend([w.values.intern(ValueRef::Node(x)), w.values.intern(ValueRef::Node(rep))]);
            rows.levels.push(t);
        }
        let mut rel = Relation::new(2, &[SWAP.to_vec()]);
        rel.load(rows, None);
        w.scc.push((calls(), rel));
        let run = |w: &mut World<'_>, head: &str, rule: usize| {
            let plan = w.plan(head);
            let b = base(&plan, rule);
            w.run(&b.steps, &b.head, eval(Tier::Fact))
                .unwrap()
                .into_iter()
                .map(|(vals, _)| vals.join(" "))
                .collect::<Vec<_>>()
        };
        assert_eq!(run(&mut w, "cyc", 0), ["n00001 n00001", "n00002 n00001"]);
        assert_eq!(run(&mut w, "onrep", 1), ["n00001", "n00002"]);
        assert_eq!(run(&mut w, "acyc", 2), ["n00000", "n00003"]);
        assert_eq!(run(&mut w, "noscc", 3), ["n00000", "n00003"]);
        assert_eq!(
            run(&mut w, "twin", 4),
            ["n00000 n00000", "n00001 n00001", "n00002 n00002", "n00003 n00003"]
        );
    }
}
