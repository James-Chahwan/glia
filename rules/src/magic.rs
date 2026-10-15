//! The magic-sets / demand rewrite for unseeded user recursion (DA.12): generalised
//! magic sets (Bancilhon-Ramakrishnan) over the rule IR, IR -> IR.
//!
//! An agent writes the unseeded closure `tc(x, z) :- tc(x, y), edge(y, z, "CALLS")`
//! and reads it from a bound site, `out(b) :- seed(a), tc(a, b)`. Bottom-up, that
//! derives every pair before the site keeps a few. The rewrite derives only the
//! pairs the sites ask for. For each binding pattern (adornment) a site uses, it
//! makes an adorned copy of the predicate's component, and a magic predicate holding
//! the demanded bound arguments guards that copy.
//!
//! **Candidates** are the recursive components of user IDB predicates, in stratum
//! order. A component is skipped when it holds an `output` (the caller reads it
//! whole), an `aggregate` (DA.5b already rejects aggregation inside recursion, so
//! this is a guard), or a predicate wider than 8 arguments (`arity`: at most 2^8
//! adornments each). Every member of a skipped component is reported with the
//! component's reason.
//!
//! **Sites** are the body occurrences of a member in rules outside the component.
//! A site's adornment (`b` / `f` per argument) is read off [`sips_order`] with
//! nothing bound. Walking that order up to the site, a literal is *kept* when it can
//! run on what the kept literals before it bind: a relation atom always; a builtin
//! once its `Must` / `Const` columns are bound; a negation once every variable is
//! bound; a comparison once both sides are bound, or an `=` whose other side is bound
//! (an assignment). An argument is `b` when it is a constant or a variable the kept
//! literals bind. A positive site with a `b` is rewritten. An all-`f` site, or a
//! negated one, reads the original predicate, which then stays next to the adorned
//! copies. A negated site is never renamed, because DA.14b attaches a negated atom's
//! absence cone to user rows by its name. A component whose sites are all
//! unrewritable is skipped as `no-bound-site`.
//!
//! **The rewrite.** Adornments propagate through the component as a fixpoint over
//! (predicate, adornment) pairs. Each rule of a demanded pair is ordered by the same
//! SIPS, with the head's `b` arguments bound. Per pair `(p, a)`:
//! - an adorned copy `__a_p__a` of each rule of `p`. Its first body atom is the
//!   magic atom `__m_p__a(<b-args of the head>)`, and each component atom is renamed
//!   to its own adorned copy;
//! - for each component atom at body position i, a propagation rule
//!   `__m_q__c(<b-args>) :- __m_p__a(<head b-args>), <kept literals before i>`. A
//!   propagation rule whose head is also one of its body atoms is dropped, because
//!   it can derive nothing new (left-linear `tc` would give `__m_tc__bf(x) :-
//!   __m_tc__bf(x)`);
//! - for each rewritten site, a seed rule `__m_p__a(<b-args>) :- <kept literals
//!   before the site>`, and the site renamed to `__a_p__a`.
//!
//! Kept literals keep their source order in every generated body. Inside a seed rule,
//! a kept literal that is itself a rewritten site is renamed too. Every generated
//! rule keeps the span of the rule it came from, so a diagnostic or witness points at
//! the user's text. A generated rule whose text was already generated is emitted once.
//! The output is the input in source order. The rules of a rewritten component are
//! dropped (or kept as written when a site still reads the original), and its whole
//! adorned block is emitted after its last rule: pairs in (PredId, adornment) order,
//! then each pair's copies in source order, each copy followed by its propagation
//! rules. Each seed rule is emitted just before the rule that holds its site.
//!
//! **Names.** `__a_<p>__<adornment>` is an adorned copy and `__m_<p>__<adornment>` a
//! magic predicate. Both take the reserved `__` prefix no user rule may use (DA.5b),
//! and `__a_` / `__m_` cannot collide. Predicate names are unique per program, and
//! an adornment is only `b` / `f`, so the last `__` splits a name back into its
//! predicate and adornment. Analysis accepts these names only through
//! `analyse_with(.., true)`, which marks them synthetic.
//!
//! **Several components.** Components are rewritten top-down: the highest stratum
//! first, with each step re-analysed (`analyse_with(.., true)`) before the next. A
//! lower component therefore sees the sites inside a higher one's adorned copies,
//! propagation and seed rules, and the demand flows down. A step whose rewritten
//! program does not analyse (a magic rule closed a negation cycle) is dropped, and
//! its members are skipped as `unstratifiable`. The steps before it and after it
//! still apply. So a program returned by [`rewrite`] always passes
//! `analyse_with(.., true)`.
//!
//! Answers: magic sets keep the answers of the rules that read a rewritten predicate,
//! never that predicate's full extension. That is why outputs are never rewritten.
//! Tiers: a magic row's tier is the tier of the kept prefix that bound it, which is a
//! subset of the site's prefix. So a reader's min-over-derivations-of-max tier is
//! unchanged (DA.18 checks it with magic on and off).
//!
//! Markers, on stderr, one per line: `[rules] magic pred=<name>/<arity>
//! adornment=<bf..> sites=<n>` per adorned pair, where `sites` counts the rewritten
//! sites outside the component that read it and is 0 when only the component's own
//! rules demand it. Then `[rules] magic skipped pred=<name>/<arity> reason=<output|
//! no-bound-site|aggregate|arity|unstratifiable>` per skipped predicate. Names are the
//! ORIGINAL user names. Everything is in PredId / source order, and no hash map is
//! iterated (`Lookup` only answers membership).

use crate::analysis::{
    Analysis, CLit, CRule, CTerm, PredId, PredInfo, PredKind, analyse_with, sips_order,
};
use crate::ir::{Atom, BindReq, Catalog, CmpOp, Head, HeadArg, Literal, Program, Rule, Term};
use crate::value::Lookup;

/// The widest predicate the rewrite adorns: at most 2^8 adornments each.
const MAX_ARITY: usize = 8;

const OUTPUT: &str = "output";
const NO_BOUND_SITE: &str = "no-bound-site";
const AGGREGATE: &str = "aggregate";
const ARITY: &str = "arity";
const UNSTRATIFIABLE: &str = "unstratifiable";

/// What the rewrite did with each recursive user predicate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MagicReport {
    /// One entry per adorned pair: the original `name/arity`, the adornment (`bf`) and
    /// the rewritten sites outside the component that read it. Sorted by PredId,
    /// then adornment.
    pub(crate) rewritten: Vec<(String, String, usize)>,
    /// One entry per recursive user predicate left as written: `name/arity` and the
    /// reason. Sorted by PredId.
    pub(crate) skipped: Vec<(String, &'static str)>,
}

impl MagicReport {
    /// The `[rules] magic` marker lines: the rewritten pairs, then the skips.
    pub(crate) fn lines(&self) -> Vec<String> {
        let rewritten = self
            .rewritten
            .iter()
            .map(|(p, a, n)| format!("[rules] magic pred={p} adornment={a} sites={n}"));
        let skipped =
            self.skipped.iter().map(|(p, r)| format!("[rules] magic skipped pred={p} reason={r}"));
        rewritten.chain(skipped).collect()
    }
}

/// Rewrite the recursive non-output user predicates of `program` (analysed as `an`
/// against `catalog`) for the bound sites that read them, and print the report's
/// marker lines. `None` when no predicate was rewritten. A returned program passes
/// `analyse_with(.., true)`.
pub(crate) fn rewrite(
    program: &Program,
    an: &Analysis,
    catalog: &Catalog,
    outputs: &[PredId],
) -> Option<(Program, MagicReport)> {
    let (rewritten, report) = demand(program, an, catalog, outputs);
    for line in report.lines() {
        eprintln!("{line}");
    }
    rewritten.map(|p| (p, report))
}

/// [`rewrite`] without the printing. The report is returned even when nothing was
/// rewritten.
fn demand(
    program: &Program,
    an: &Analysis,
    catalog: &Catalog,
    outputs: &[PredId],
) -> (Option<Program>, MagicReport) {
    if !describes(program, an) {
        return (None, MagicReport::default());
    }
    let info = |p: PredId| an.preds.get(p as usize);
    let mut skipped: Vec<(PredId, &'static str)> = Vec::new();
    let mut todo: Vec<&[PredId]> = Vec::new();
    for st in &an.strata {
        let user = st
            .preds
            .iter()
            .all(|&p| info(p).is_some_and(|i| i.kind == PredKind::Idb && !i.synthetic));
        if !st.recursive || !user {
            continue;
        }
        let reason = if st.preds.iter().any(|p| outputs.contains(p)) {
            Some(OUTPUT)
        } else if st.aggregate {
            Some(AGGREGATE)
        } else if st.preds.iter().any(|&p| info(p).is_some_and(|i| i.arity > MAX_ARITY)) {
            Some(ARITY)
        } else {
            None
        };
        match reason {
            Some(r) => skipped.extend(st.preds.iter().map(|&p| (p, r))),
            None => todo.push(&st.preds),
        }
    }
    let mut rewritten: Vec<(PredId, String, usize)> = Vec::new();
    let mut current: Option<(Program, Analysis)> = None;
    for &members in todo.iter().rev() {
        let names: Vec<(&str, usize)> =
            members.iter().filter_map(|&p| info(p)).map(|i| (i.name.as_str(), i.arity)).collect();
        let (prog, cur) = current.as_ref().map_or((program, an), |(p, a)| (p, a));
        let step = component(prog, cur, catalog, &names).and_then(|step| {
            analyse_with(&step.program, catalog, true)
                .map(|a| (step, a))
                .map_err(|_| UNSTRATIFIABLE)
        });
        match step {
            Ok((step, next)) => {
                for (m, adornment, sites) in step.pairs {
                    if let Some(&p) = members.get(m) {
                        rewritten.push((p, adornment, sites));
                    }
                }
                current = Some((step.program, next));
            }
            Err(reason) => skipped.extend(members.iter().map(|&p| (p, reason))),
        }
    }
    skipped.sort_by_key(|&(p, _)| p);
    rewritten.sort();
    let sig = |p: PredId| info(p).map_or_else(String::new, PredInfo::signature);
    let report = MagicReport {
        rewritten: rewritten.into_iter().map(|(p, a, n)| (sig(p), a, n)).collect(),
        skipped: skipped.into_iter().map(|(p, r)| (sig(p), r)).collect(),
    };
    (current.map(|(p, _)| p), report)
}

/// Is `an` an analysis of `program`: one compiled rule per rule, in order, each with
/// one compiled literal per body literal? Every `Ok` analysis of the program is.
fn describes(program: &Program, an: &Analysis) -> bool {
    an.rules.len() == program.rules.len()
        && an
            .rules
            .iter()
            .zip(&program.rules)
            .enumerate()
            .all(|(i, (cr, r))| cr.index == i && cr.body.len() == r.body.len())
}

/// One component's rewrite: the program, and per adorned pair the member's index,
/// the adornment and its rewritten sites.
struct Step {
    program: Program,
    pairs: Vec<(usize, String, usize)>,
}

/// One (predicate, adornment) pair the rewrite demands.
struct Pair {
    pred: PredId,
    adornment: Vec<bool>,
    /// Rewritten sites outside the component that read it.
    sites: usize,
    /// Per rule of `pred` in source order, its program index and its component atoms.
    copies: Vec<(usize, Vec<Demand>)>,
}

/// A demanded atom: a component atom inside a copy, or a rewritten site.
struct Demand {
    /// Its body position.
    lit: usize,
    /// The pair it reads (an index into the component's pairs).
    pair: usize,
    /// The body positions its magic rule keeps, ascending.
    kept: Vec<usize>,
}

/// Rewrite the component whose members are `members` (`name`, arity) in `prog`
/// (analysed as `an`), or say why not.
fn component(
    prog: &Program,
    an: &Analysis,
    catalog: &Catalog,
    members: &[(&str, usize)],
) -> Result<Step, &'static str> {
    let scc: Vec<PredId> = members.iter().filter_map(|&(n, a)| find(an, n, a)).collect();
    if scc.len() != members.len() {
        // Rewriting a higher component never drops a lower one's rules, so this does
        // not happen; a member the program no longer defines has no site.
        return Err(NO_BOUND_SITE);
    }
    let in_scc = |p: &PredId| scc.contains(p);
    let mut pairs: Vec<Pair> = Vec::new();
    // Per rewritten site: its rule's program index and its demand.
    let mut sites: Vec<(usize, Demand)> = Vec::new();
    let mut keep_original = false;
    for cr in an.rules.iter().filter(|r| !in_scc(&r.head)) {
        if !cr.body.iter().any(|l| matches!(l, CLit::Pos(p, ..) | CLit::Neg(p, ..) if in_scc(p))) {
            continue;
        }
        let order = sips_order(cr, &an.preds, catalog, &[], None, &|_| None);
        for (i, lit) in cr.body.iter().enumerate() {
            match lit {
                CLit::Neg(p, ..) if in_scc(p) => keep_original = true,
                CLit::Pos(p, terms, _) if in_scc(p) => {
                    let unbound = vec![false; usize::from(cr.nvars)];
                    let (kept, adornment) =
                        prefix(cr, &an.preds, catalog, &order, i, unbound, terms);
                    if adornment.contains(&true) {
                        let pair = pair_of(&mut pairs, *p, adornment);
                        pairs[pair].sites += 1;
                        sites.push((cr.index, Demand { lit: i, pair, kept }));
                    } else {
                        keep_original = true;
                    }
                }
                _ => {}
            }
        }
    }
    if sites.is_empty() {
        return Err(NO_BOUND_SITE);
    }
    // The fixpoint over (predicate, adornment) pairs: finite, since every member has at
    // most 2^MAX_ARITY adornments.
    let mut k = 0;
    while k < pairs.len() {
        let (pred, adornment) = (pairs[k].pred, pairs[k].adornment.clone());
        let mut copies = Vec::new();
        for cr in an.rules.iter().filter(|r| r.head == pred) {
            let mut bound = vec![false; usize::from(cr.nvars)];
            for (t, _) in cr.head_terms.iter().zip(&adornment).filter(|(_, b)| **b) {
                bind_term(t, &mut bound);
            }
            let order = sips_order(cr, &an.preds, catalog, &bound, None, &|_| None);
            let mut atoms = Vec::new();
            for (i, lit) in cr.body.iter().enumerate() {
                if let CLit::Pos(q, terms, _) = lit
                    && in_scc(q)
                {
                    let (kept, c) = prefix(cr, &an.preds, catalog, &order, i, bound.clone(), terms);
                    atoms.push(Demand { lit: i, pair: pair_of(&mut pairs, *q, c), kept });
                }
            }
            copies.push((cr.index, atoms));
        }
        pairs[k].copies = copies;
        k += 1;
    }

    let name = |p: PredId| an.preds.get(p as usize).map_or("", |i| i.name.as_str());
    let adorned: Vec<String> =
        pairs.iter().map(|q| synthetic('a', name(q.pred), &q.adornment)).collect();
    let magic: Vec<String> =
        pairs.iter().map(|q| synthetic('m', name(q.pred), &q.adornment)).collect();
    let mut block: Vec<usize> = (0..pairs.len()).collect();
    block.sort_by_key(|&k| (pairs[k].pred, adornment_text(&pairs[k].adornment)));
    let last = an.rules.iter().filter(|r| in_scc(&r.head)).map(|r| r.index).max();
    let mut out = Emit { rules: Vec::with_capacity(prog.rules.len()), seen: Lookup::new() };
    for (ri, rule) in prog.rules.iter().enumerate() {
        if an.rules.get(ri).is_some_and(|cr| in_scc(&cr.head)) {
            if keep_original {
                out.rules.push(rule.clone());
            }
            if Some(ri) != last {
                continue;
            }
            for &k in &block {
                for (rj, atoms) in &pairs[k].copies {
                    let Some(r) = prog.rules.get(*rj) else {
                        continue;
                    };
                    let names = renames(r.body.len(), atoms, &adorned);
                    let guard = Literal::Pos(Atom {
                        pred: magic[k].clone(),
                        args: head_bound(&r.head, &pairs[k].adornment),
                        span: r.head.span,
                    });
                    let mut body = vec![guard.clone()];
                    body.extend((0..r.body.len()).filter_map(|j| renamed(r, j, &names)));
                    let head = Head { pred: adorned[k].clone(), ..r.head.clone() };
                    out.generated(Rule { head, body, span: r.span });
                    for d in atoms {
                        let mut body = vec![guard.clone()];
                        body.extend(d.kept.iter().filter_map(|&j| renamed(r, j, &names)));
                        out.magic(r, d, &pairs, &magic, body);
                    }
                }
            }
            continue;
        }
        let here: Vec<&Demand> = sites.iter().filter(|(r, _)| *r == ri).map(|(_, d)| d).collect();
        if here.is_empty() {
            out.rules.push(rule.clone());
            continue;
        }
        let names = renames(rule.body.len(), here.iter().copied(), &adorned);
        for d in &here {
            let body = d.kept.iter().filter_map(|&j| renamed(rule, j, &names)).collect();
            out.magic(rule, d, &pairs, &magic, body);
        }
        let body = (0..rule.body.len()).filter_map(|j| renamed(rule, j, &names)).collect();
        out.rules.push(Rule { body, ..rule.clone() });
    }
    let member = |p: PredId| scc.iter().position(|&q| q == p);
    let report = pairs
        .iter()
        .filter_map(|q| member(q.pred).map(|m| (m, adornment_text(&q.adornment), q.sites)))
        .collect();
    Ok(Step { program: Program { sources: prog.sources.clone(), rules: out.rules }, pairs: report })
}

/// The rewritten program under construction.
struct Emit {
    rules: Vec<Rule>,
    /// The text of every generated rule so far.
    seen: Lookup<String, ()>,
}

impl Emit {
    /// Push a generated rule unless the same text was generated before.
    fn generated(&mut self, rule: Rule) {
        let text = rule.to_string();
        if !self.seen.contains_key(&text) {
            self.seen.insert(text, ());
            self.rules.push(rule);
        }
    }

    /// Push the magic rule for demand `d` of `rule`: head `__m_q__c(<b-args of the
    /// demanded atom>)`, then `body`. It is dropped when its head is one of its body
    /// atoms.
    fn magic(
        &mut self,
        rule: &Rule,
        d: &Demand,
        pairs: &[Pair],
        magic: &[String],
        body: Vec<Literal>,
    ) {
        let (Some(Literal::Pos(atom)), Some(pair)) = (rule.body.get(d.lit), pairs.get(d.pair))
        else {
            return;
        };
        let args: Vec<HeadArg> = atom
            .args
            .iter()
            .zip(&pair.adornment)
            .filter(|(_, b)| **b)
            .map(|(t, _)| HeadArg::Term(t.clone()))
            .collect();
        let head = Head { pred: magic[d.pair].clone(), args, span: atom.span };
        let restates = body.iter().any(|l| match l {
            Literal::Pos(a) => {
                a.pred == head.pred
                    && a.args.len() == head.args.len()
                    && a.args.iter().zip(&head.args).all(|(t, h)| t.to_string() == h.to_string())
            }
            Literal::Neg(_) | Literal::Cmp(_) => false,
        });
        if !restates {
            self.generated(Rule { head, body, span: rule.span });
        }
    }
}

/// `__a_<p>__<adornment>` or `__m_<p>__<adornment>`.
fn synthetic(tag: char, pred: &str, adornment: &[bool]) -> String {
    format!("__{tag}_{pred}__{}", adornment_text(adornment))
}

/// `b` per bound argument, `f` per free one.
fn adornment_text(adornment: &[bool]) -> String {
    adornment.iter().map(|&b| if b { 'b' } else { 'f' }).collect()
}

/// The IDB predicate `name/arity` of `an`.
fn find(an: &Analysis, name: &str, arity: usize) -> Option<PredId> {
    let p = an
        .preds
        .iter()
        .position(|i| i.kind == PredKind::Idb && i.name == name && i.arity == arity)?;
    PredId::try_from(p).ok()
}

/// The index of pair (`pred`, `adornment`), added when new.
fn pair_of(pairs: &mut Vec<Pair>, pred: PredId, adornment: Vec<bool>) -> usize {
    if let Some(k) = pairs.iter().position(|q| q.pred == pred && q.adornment == adornment) {
        return k;
    }
    pairs.push(Pair { pred, adornment, sites: 0, copies: Vec::new() });
    pairs.len() - 1
}

/// The head's arguments at the adornment's `b` positions, as atom terms.
fn head_bound(head: &Head, adornment: &[bool]) -> Vec<Term> {
    head.args
        .iter()
        .zip(adornment)
        .filter(|(_, b)| **b)
        .filter_map(|(a, _)| match a {
            HeadArg::Term(t) => Some(t.clone()),
            HeadArg::Agg(_) => None,
        })
        .collect()
}

/// Per body position of a rule with `n` literals, the adorned name a demanded atom
/// takes there.
fn renames<'d>(
    n: usize,
    demands: impl IntoIterator<Item = &'d Demand>,
    adorned: &[String],
) -> Vec<Option<String>> {
    let mut names = vec![None; n];
    for d in demands {
        if let (Some(slot), Some(name)) = (names.get_mut(d.lit), adorned.get(d.pair)) {
            *slot = Some(name.clone());
        }
    }
    names
}

/// Body literal `j` of `rule`, renamed when `names` gives it an adorned name.
fn renamed(rule: &Rule, j: usize, names: &[Option<String>]) -> Option<Literal> {
    let lit = rule.body.get(j)?;
    Some(match (lit, names.get(j).and_then(Option::as_ref)) {
        (Literal::Pos(a), Some(name)) => Literal::Pos(Atom { pred: name.clone(), ..a.clone() }),
        _ => lit.clone(),
    })
}

/// Walk `order` (a [`sips_order`] of `cr`) up to body position `site`, from the
/// variables `bound` marks, keeping each literal that can run on what the kept
/// literals before it bind (the module doc's rule). Returns the kept positions,
/// ascending, and the adornment of `site_terms` there.
fn prefix(
    cr: &CRule,
    preds: &[PredInfo],
    catalog: &Catalog,
    order: &[usize],
    site: usize,
    mut bound: Vec<bool>,
    site_terms: &[CTerm],
) -> (Vec<usize>, Vec<bool>) {
    let mut kept = Vec::new();
    for &i in order.iter().take_while(|&&i| i != site) {
        let Some(lit) = cr.body.get(i) else { continue };
        let runs = match lit {
            CLit::Pos(p, terms, _) => terms.iter().enumerate().all(|(c, t)| {
                match preds.get(*p as usize).and_then(|info| info.bind_req(catalog, c)) {
                    Some(BindReq::Must | BindReq::Const) => is_bound(t, &bound),
                    Some(BindReq::May) | None => true,
                }
            }),
            CLit::Neg(_, terms, _) => {
                terms.iter().all(|t| !matches!(t, CTerm::Var(_)) || is_bound(t, &bound))
            }
            CLit::Cmp(op, l, r, _) => {
                let (lb, rb) = (is_bound(l, &bound), is_bound(r, &bound));
                (lb && rb)
                    || (*op == CmpOp::Eq
                        && ((lb && matches!(r, CTerm::Var(_)))
                            || (rb && matches!(l, CTerm::Var(_)))))
            }
        };
        if !runs {
            continue;
        }
        kept.push(i);
        match lit {
            CLit::Pos(_, terms, _) => terms.iter().for_each(|t| bind_term(t, &mut bound)),
            CLit::Cmp(CmpOp::Eq, l, r, _) => {
                bind_term(l, &mut bound);
                bind_term(r, &mut bound);
            }
            CLit::Neg(..) | CLit::Cmp(..) => {}
        }
    }
    kept.sort_unstable();
    (kept, site_terms.iter().map(|t| is_bound(t, &bound)).collect())
}

/// A constant, or a variable `bound` marks. `_` is never bound.
fn is_bound(t: &CTerm, bound: &[bool]) -> bool {
    match t {
        CTerm::Const(_) => true,
        CTerm::Var(v) => bound.get(usize::from(*v)).copied().unwrap_or(false),
        CTerm::Wild => false,
    }
}

/// Mark `t` bound when it is a variable.
fn bind_term(t: &CTerm, bound: &mut [bool]) {
    if let CTerm::Var(v) = t
        && let Some(b) = bound.get_mut(usize::from(*v))
    {
        *b = true;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::analysis::{ConstVal, EdbPred, analyse};
    use glia_activation::profile::Registries;
    use glia_core::{CellTypeId, EdgeCategoryId, NodeKindId};

    const REG: Registries = Registries {
        node_kinds: &[(NodeKindId(1), "FUNCTION")],
        edge_categories: &[
            (EdgeCategoryId(1), "CALLS"),
            (EdgeCategoryId(2), "USES"),
            (EdgeCategoryId(3), "IMPORTS"),
        ],
        cell_types: &[(CellTypeId(1), "CODE")],
    };

    /// Two CALLS cycles (a b c, x y) with tails, USES / IMPORTS edges for the
    /// same-generation program, and a node (m) reaching into the first cycle.
    const GRAPH: &[(&str, &str, &str)] = &[
        ("a", "b", "CALLS"),
        ("b", "c", "CALLS"),
        ("c", "a", "CALLS"),
        ("c", "d", "CALLS"),
        ("d", "e", "CALLS"),
        ("x", "y", "CALLS"),
        ("y", "x", "CALLS"),
        ("m", "a", "CALLS"),
        ("a", "u", "USES"),
        ("u", "v", "USES"),
        ("d", "w", "USES"),
        ("e", "x", "USES"),
        ("b", "c", "IMPORTS"),
        ("v", "w", "IMPORTS"),
        ("u", "e", "IMPORTS"),
        ("v", "v", "IMPORTS"),
    ];

    fn catalog() -> Catalog {
        Catalog::core(REG, &[EdgeCategoryId(1)])
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

    fn edge(a: &str, b: &str, cat: &str) -> Literal {
        pos("edge", vec![v(a), v(b), s(cat)])
    }

    fn rule(head: &str, args: Vec<Term>, body: Vec<Literal>) -> Rule {
        Rule::new(head, args.into_iter().map(HeadArg::Term).collect(), body)
    }

    fn fact(p: &str, consts: &[&str]) -> Rule {
        Rule::fact(p, consts.iter().map(|c| s(c)).collect())
    }

    fn program(rules: Vec<Rule>) -> Program {
        let mut p = Program::new();
        p.add_source("rules");
        p.rules = rules;
        p
    }

    /// Left-linear `tc(x, y) :- edge(x, y, "CALLS"). tc(x, z) :- tc(x, y), edge(y, z, "CALLS").`
    fn tc_left() -> Vec<Rule> {
        vec![
            rule("tc", vec![v("x"), v("y")], vec![edge("x", "y", "CALLS")]),
            rule(
                "tc",
                vec![v("x"), v("z")],
                vec![pos("tc", vec![v("x"), v("y")]), edge("y", "z", "CALLS")],
            ),
        ]
    }

    /// Right-linear `tc(x, y) :- edge(x, y, "CALLS"). tc(x, z) :- edge(x, y, "CALLS"), tc(y, z).`
    fn tc_right() -> Vec<Rule> {
        vec![
            rule("tc", vec![v("x"), v("y")], vec![edge("x", "y", "CALLS")]),
            rule(
                "tc",
                vec![v("x"), v("z")],
                vec![edge("x", "y", "CALLS"), pos("tc", vec![v("y"), v("z")])],
            ),
        ]
    }

    /// `out(b) :- seed(a), tc(a, b).`
    fn out_bound() -> Rule {
        rule("out", vec![v("b")], vec![pos("seed", vec![v("a")]), pos("tc", vec![v("a"), v("b")])])
    }

    fn analysed(p: &Program) -> Analysis {
        match analyse(p, &catalog()) {
            Ok(a) => a,
            Err(e) => panic!("expected an analysis, got: {e}"),
        }
    }

    fn id_of(a: &Analysis, name: &str) -> PredId {
        let p = a.preds.iter().position(|p| p.kind == PredKind::Idb && p.name == name);
        p.map(|p| p as PredId).unwrap_or_else(|| panic!("no predicate {name}"))
    }

    /// [`demand`] over `p` with `outputs` named; a rewritten program must analyse.
    fn run(p: &Program, outputs: &[&str]) -> (Option<Program>, MagicReport) {
        let an = analysed(p);
        let ids: Vec<PredId> = outputs.iter().map(|o| id_of(&an, o)).collect();
        let (out, report) = demand(p, &an, &catalog(), &ids);
        if let Some(q) = &out
            && let Err(e) = analyse_with(q, &catalog(), true)
        {
            panic!("the rewrite does not analyse: {e}\n{q}");
        }
        (out, report)
    }

    /// The rewritten program's text, and the report.
    fn rewritten(p: &Program, outputs: &[&str]) -> (String, MagicReport) {
        match run(p, outputs) {
            (Some(q), report) => (q.to_string(), report),
            (None, report) => panic!("expected a rewrite, got none: {report:?}"),
        }
    }

    fn pairs(entries: &[(&str, &str, usize)]) -> Vec<(String, String, usize)> {
        entries.iter().map(|(p, a, n)| ((*p).to_string(), (*a).to_string(), *n)).collect()
    }

    fn skips(entries: &[(&str, &'static str)]) -> Vec<(String, &'static str)> {
        entries.iter().map(|(p, r)| ((*p).to_string(), *r)).collect()
    }

    // ------------------------------------------------------------------------
    // A naive evaluator, to check the rewrite keeps the answers
    // ------------------------------------------------------------------------

    type Rows = BTreeSet<Vec<String>>;

    /// Every user IDB predicate's rows: strata in order, each rule body joined in its
    /// sips order until nothing grows, `edge/3` read from `edges`. Also the number
    /// of derived rows over every IDB predicate, synthetic ones included. Builtins
    /// are not supported.
    fn naive(p: &Program, edges: &[(&str, &str, &str)]) -> (BTreeMap<String, Rows>, usize) {
        let cat = catalog();
        let an = match analyse_with(p, &cat, true) {
            Ok(a) => a,
            Err(e) => panic!("naive: {e}"),
        };
        let mut rel: Vec<Rows> = vec![Rows::new(); an.preds.len()];
        for (i, info) in an.preds.iter().enumerate() {
            if info.kind == PredKind::Edb(EdbPred::Edge) {
                rel[i] = edges
                    .iter()
                    .map(|(a, b, c)| vec![a.to_string(), b.to_string(), c.to_string()])
                    .collect();
            }
        }
        for st in &an.strata {
            loop {
                let mut grew = false;
                for cr in an.rules.iter().filter(|r| st.preds.contains(&r.head)) {
                    let order = sips_order(cr, &an.preds, &cat, &[], None, &|_| None);
                    let mut found = Vec::new();
                    join(cr, &order, &mut vec![None; usize::from(cr.nvars)], &rel, &mut found);
                    for row in found {
                        grew |= rel[cr.head as usize].insert(row);
                    }
                }
                if !grew {
                    break;
                }
            }
        }
        let idb = |i: &PredInfo| i.kind == PredKind::Idb;
        let derived = an.preds.iter().zip(&rel).filter(|(i, _)| idb(i)).map(|(_, r)| r.len()).sum();
        let user = an
            .preds
            .iter()
            .zip(&rel)
            .filter(|(i, _)| idb(i) && !i.synthetic)
            .map(|(i, r)| (i.signature(), r.clone()))
            .collect();
        (user, derived)
    }

    fn value(t: &CTerm, b: &[Option<String>]) -> Option<String> {
        match t {
            CTerm::Var(x) => b[usize::from(*x)].clone(),
            CTerm::Const(ConstVal::Str(s)) => Some(s.clone()),
            CTerm::Const(ConstVal::Int(i)) => Some(i.to_string()),
            CTerm::Const(ConstVal::CatSet(_)) | CTerm::Wild => None,
        }
    }

    fn unify(terms: &[CTerm], row: &[String], b: &mut [Option<String>]) -> bool {
        for (t, val) in terms.iter().zip(row) {
            match t {
                CTerm::Wild => {}
                CTerm::Var(x) => match &b[usize::from(*x)] {
                    Some(have) if have != val => return false,
                    Some(_) => {}
                    None => b[usize::from(*x)] = Some(val.clone()),
                },
                c => {
                    if value(c, b).as_ref() != Some(val) {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn join(
        cr: &CRule,
        order: &[usize],
        b: &mut Vec<Option<String>>,
        rel: &[Rows],
        out: &mut Vec<Vec<String>>,
    ) {
        let Some((&i, rest)) = order.split_first() else {
            out.push(cr.head_terms.iter().map(|t| value(t, b).expect("a bound head")).collect());
            return;
        };
        match &cr.body[i] {
            CLit::Pos(p, terms, _) => {
                for row in &rel[*p as usize] {
                    let saved = b.clone();
                    if unify(terms, row, b) {
                        join(cr, rest, b, rel, out);
                    }
                    *b = saved;
                }
            }
            CLit::Neg(p, terms, _) => {
                if !rel[*p as usize].iter().any(|row| unify(terms, row, &mut b.clone())) {
                    join(cr, rest, b, rel, out);
                }
            }
            CLit::Cmp(op, l, r, _) => match (value(l, b), value(r, b)) {
                (Some(x), Some(y)) => {
                    let holds = match op {
                        CmpOp::Eq => x == y,
                        CmpOp::Ne => x != y,
                        CmpOp::Lt => x < y,
                        CmpOp::Le => x <= y,
                        CmpOp::Gt => x > y,
                        CmpOp::Ge => x >= y,
                    };
                    if holds {
                        join(cr, rest, b, rel, out);
                    }
                }
                (None, Some(y)) | (Some(y), None) if *op == CmpOp::Eq => {
                    let saved = b.clone();
                    let mut set = |t: &CTerm| {
                        if let CTerm::Var(x) = t
                            && b[usize::from(*x)].is_none()
                        {
                            b[usize::from(*x)] = Some(y.clone());
                        }
                    };
                    set(l);
                    set(r);
                    join(cr, rest, b, rel, out);
                    *b = saved;
                }
                _ => {}
            },
        }
    }

    /// The rewrite of `p` (for `outputs`) gives every output the rows the original
    /// does; returns the derived-row counts without and with the rewrite.
    fn same_answers(p: &Program, outputs: &[&str]) -> (usize, usize) {
        let (Some(q), _) = run(p, outputs) else { panic!("expected a rewrite") };
        let (before, all_before) = naive(p, GRAPH);
        let (after, all_after) = naive(&q, GRAPH);
        for o in outputs {
            let key = before.keys().find(|k| k.starts_with(&format!("{o}/"))).cloned();
            let key = key.unwrap_or_else(|| panic!("no output {o}"));
            assert!(!before[&key].is_empty(), "{key} has no rows: the check proves nothing");
            assert_eq!(before.get(&key), after.get(&key), "{key} changed under\n{q}");
        }
        (all_before, all_after)
    }

    // ------------------------------------------------------------------------
    // The acceptance tests
    // ------------------------------------------------------------------------

    #[test]
    fn rewrites_bound_tc() {
        let mut rules = tc_left();
        rules.push(out_bound());
        rules.push(fact("seed", &["a"]));
        let p = program(rules);
        let (text, report) = rewritten(&p, &["out"]);
        let expected = r#"__a_tc__bf(x, y) :- __m_tc__bf(x), edge(x, y, "CALLS").
__a_tc__bf(x, z) :- __m_tc__bf(x), __a_tc__bf(x, y), edge(y, z, "CALLS").
__m_tc__bf(a) :- seed(a).
out(b) :- seed(a), __a_tc__bf(a, b).
seed("a").
"#;
        assert_eq!(text, expected);
        // The propagation rule `__m_tc__bf(x) :- __m_tc__bf(x).` restates its head.
        assert!(!text.contains("__m_tc__bf(x) :- __m_tc__bf(x)."));
        assert_eq!(report.rewritten, pairs(&[("tc/2", "bf", 1)]));
        assert!(report.skipped.is_empty());
        // The public entry point returns the same program and report (and prints them).
        let an = analysed(&p);
        let printed = rewrite(&p, &an, &catalog(), &[id_of(&an, "out")]);
        assert_eq!(printed.map(|(q, r)| (q.to_string(), r)), Some((text, report)));
        // The answers hold, and the seeded copy derives under half the rows.
        let (without, with) = same_answers(&p, &["out"]);
        assert!(with * 2 < without, "derived {with} with magic, {without} without");
    }

    #[test]
    fn right_linear_adornment() {
        let mut rules = tc_right();
        rules.push(out_bound());
        rules.push(fact("seed", &["a"]));
        let p = program(rules);
        let (text, report) = rewritten(&p, &["out"]);
        let expected = r#"__a_tc__bf(x, y) :- __m_tc__bf(x), edge(x, y, "CALLS").
__a_tc__bf(x, z) :- __m_tc__bf(x), edge(x, y, "CALLS"), __a_tc__bf(y, z).
__m_tc__bf(y) :- __m_tc__bf(x), edge(x, y, "CALLS").
__m_tc__bf(a) :- seed(a).
out(b) :- seed(a), __a_tc__bf(a, b).
seed("a").
"#;
        assert_eq!(text, expected);
        assert_eq!(report.rewritten, pairs(&[("tc/2", "bf", 1)]));
        same_answers(&p, &["out"]);
    }

    #[test]
    fn output_is_skipped() {
        let mut rules = tc_left();
        rules.push(out_bound());
        rules.push(fact("seed", &["a"]));
        let p = program(rules);
        let (q, report) = run(&p, &["tc"]);
        assert!(q.is_none());
        assert!(report.rewritten.is_empty());
        assert_eq!(report.skipped, skips(&[("tc/2", OUTPUT)]));
        // The public entry point agrees.
        let an = analysed(&p);
        assert!(rewrite(&p, &an, &catalog(), &[id_of(&an, "tc")]).is_none());
    }

    #[test]
    fn free_site_keeps_original() {
        let mut rules = tc_left();
        rules.push(out_bound());
        rules.push(rule("all", vec![v("a"), v("b")], vec![pos("tc", vec![v("a"), v("b")])]));
        rules.push(fact("seed", &["a"]));
        let p = program(rules);
        let (text, report) = rewritten(&p, &["out", "all"]);
        let expected = r#"tc(x, y) :- edge(x, y, "CALLS").
tc(x, z) :- tc(x, y), edge(y, z, "CALLS").
__a_tc__bf(x, y) :- __m_tc__bf(x), edge(x, y, "CALLS").
__a_tc__bf(x, z) :- __m_tc__bf(x), __a_tc__bf(x, y), edge(y, z, "CALLS").
__m_tc__bf(a) :- seed(a).
out(b) :- seed(a), __a_tc__bf(a, b).
all(a, b) :- tc(a, b).
seed("a").
"#;
        assert_eq!(text, expected);
        assert_eq!(report.rewritten, pairs(&[("tc/2", "bf", 1)]));
        same_answers(&p, &["out", "all"]);

        // A negated site reads the original by name too, even fully bound.
        let mut rules = tc_left();
        rules.push(out_bound());
        rules.push(rule(
            "loose",
            vec![v("a")],
            vec![pos("seed", vec![v("a")]), neg("tc", vec![v("a"), s("e")])],
        ));
        rules.push(fact("seed", &["a"]));
        rules.push(fact("seed", &["x"]));
        let p = program(rules);
        let (text, _) = rewritten(&p, &["out", "loose"]);
        assert!(text.starts_with("tc(x, y) :- edge(x, y, \"CALLS\").\n"), "{text}");
        assert!(text.contains("loose(a) :- seed(a), !tc(a, \"e\").\n"), "{text}");
        same_answers(&p, &["out", "loose"]);
    }

    #[test]
    fn two_adornments() {
        let mut rules = tc_left();
        rules.push(out_bound());
        rules.push(rule(
            "into",
            vec![v("a")],
            vec![pos("seed", vec![v("b")]), pos("tc", vec![v("a"), v("b")])],
        ));
        rules.push(fact("seed", &["a"]));
        rules.push(fact("seed", &["x"]));
        let p = program(rules);
        let (text, report) = rewritten(&p, &["out", "into"]);
        let expected = r#"__a_tc__bf(x, y) :- __m_tc__bf(x), edge(x, y, "CALLS").
__a_tc__bf(x, z) :- __m_tc__bf(x), __a_tc__bf(x, y), edge(y, z, "CALLS").
__a_tc__fb(x, y) :- __m_tc__fb(y), edge(x, y, "CALLS").
__a_tc__fb(x, z) :- __m_tc__fb(z), __a_tc__fb(x, y), edge(y, z, "CALLS").
__m_tc__fb(y) :- __m_tc__fb(z), edge(y, z, "CALLS").
__m_tc__bf(a) :- seed(a).
out(b) :- seed(a), __a_tc__bf(a, b).
__m_tc__fb(b) :- seed(b).
into(a) :- seed(b), __a_tc__fb(a, b).
seed("a").
seed("x").
"#;
        assert_eq!(text, expected);
        assert_eq!(report.rewritten, pairs(&[("tc/2", "bf", 1), ("tc/2", "fb", 1)]));
        same_answers(&p, &["out", "into"]);
    }

    #[test]
    fn negation_before_site_stays_out_unless_bound() {
        let mut rules = tc_left();
        // `!bad(a)` and `a != "z"` are bound before the site: they enter the seed rule.
        rules.push(rule(
            "out",
            vec![v("b")],
            vec![
                pos("seed", vec![v("a")]),
                neg("bad", vec![v("a")]),
                Literal::cmp(CmpOp::Ne, v("a"), s("z")),
                pos("tc", vec![v("a"), v("b")]),
                Literal::cmp(CmpOp::Ne, v("b"), s("q")),
            ],
        ));
        // `!bad(b)` precedes the site in the source but binds only after it: it stays out.
        rules.push(rule(
            "out2",
            vec![v("b")],
            vec![
                pos("seed", vec![v("a")]),
                neg("bad", vec![v("b")]),
                pos("tc", vec![v("a"), v("b")]),
            ],
        ));
        rules.push(fact("seed", &["a"]));
        rules.push(fact("seed", &["x"]));
        rules.push(fact("bad", &["x"]));
        rules.push(fact("bad", &["c"]));
        let p = program(rules);
        let (text, report) = rewritten(&p, &["out", "out2"]);
        let expected = r#"__a_tc__bf(x, y) :- __m_tc__bf(x), edge(x, y, "CALLS").
__a_tc__bf(x, z) :- __m_tc__bf(x), __a_tc__bf(x, y), edge(y, z, "CALLS").
__m_tc__bf(a) :- seed(a), !bad(a), a != "z".
out(b) :- seed(a), !bad(a), a != "z", __a_tc__bf(a, b), b != "q".
__m_tc__bf(a) :- seed(a).
out2(b) :- seed(a), !bad(b), __a_tc__bf(a, b).
seed("a").
seed("x").
bad("x").
bad("c").
"#;
        assert_eq!(text, expected);
        assert_eq!(report.rewritten, pairs(&[("tc/2", "bf", 2)]));
        same_answers(&p, &["out", "out2"]);
    }

    #[test]
    fn deterministic() {
        let mut rules = tc_left();
        rules.push(out_bound());
        rules.push(rule(
            "into",
            vec![v("a")],
            vec![pos("seed", vec![v("b")]), pos("tc", vec![v("a"), v("b")])],
        ));
        rules.push(fact("seed", &["a"]));
        let p = program(rules);
        let (one, r1) = rewritten(&p, &["out", "into"]);
        let (two, r2) = rewritten(&p, &["out", "into"]);
        assert_eq!(one, two);
        assert_eq!(r1, r2);
        assert_eq!(r1.lines(), r2.lines());
    }

    // ------------------------------------------------------------------------
    // Skips, markers and the wider shapes
    // ------------------------------------------------------------------------

    #[test]
    fn free_sites_only_are_no_bound_site() {
        let mut rules = tc_left();
        rules.push(rule("all", vec![v("a"), v("b")], vec![pos("tc", vec![v("a"), v("b")])]));
        let (q, report) = run(&program(rules), &["all"]);
        assert!(q.is_none());
        assert_eq!(report.skipped, skips(&[("tc/2", NO_BOUND_SITE)]));
    }

    #[test]
    fn negation_cycle_is_unstratifiable() {
        // DA.14b's fallback program: `__m_tc__bf` would read `!r` while r reads
        // `__a_tc__bf`, a negation inside recursion.
        let mut rules = tc_left();
        rules.push(rule(
            "q",
            vec![v("y")],
            vec![
                pos("seed", vec![v("x")]),
                neg("r", vec![v("x")]),
                pos("tc", vec![v("x"), v("y")]),
            ],
        ));
        rules.push(rule(
            "r",
            vec![v("x")],
            vec![pos("seed2", vec![v("x")]), pos("tc", vec![v("x"), v("_z")])],
        ));
        rules.push(fact("seed", &["a"]));
        rules.push(fact("seed2", &["x"]));
        let (q, report) = run(&program(rules), &["q"]);
        assert!(q.is_none());
        assert!(report.rewritten.is_empty());
        assert_eq!(report.skipped, skips(&[("tc/2", UNSTRATIFIABLE)]));
    }

    #[test]
    fn arity_and_aggregate_guards() {
        // A 9-ary rotation: recursive, read from a bound site, too wide to adorn.
        let xs: Vec<Term> = (1..=9).map(|i| v(&format!("x{i}"))).collect();
        let mut rotated = xs[1..].to_vec();
        rotated.push(xs[0].clone());
        let mut site = vec![v("a"), v("b")];
        site.extend((0..7).map(|_| Term::wild()));
        let p = program(vec![
            fact("w", &["1", "2", "3", "4", "5", "6", "7", "8", "9"]),
            rule("w", xs, vec![pos("w", rotated)]),
            rule("out", vec![v("b")], vec![pos("seed", vec![v("a")]), pos("w", site)]),
            fact("seed", &["1"]),
        ]);
        let (q, report) = run(&p, &["out"]);
        assert!(q.is_none());
        assert_eq!(report.skipped, skips(&[("w/9", ARITY)]));

        // Analysis rejects aggregation inside recursion, so the aggregate guard only
        // fires on an analysis marked by hand.
        let mut rules = tc_left();
        rules.push(out_bound());
        rules.push(fact("seed", &["a"]));
        let p = program(rules);
        let mut an = analysed(&p);
        let tc = id_of(&an, "tc");
        let k = an.preds[tc as usize].stratum;
        an.strata[k].aggregate = true;
        let (q, report) = demand(&p, &an, &catalog(), &[id_of(&an, "out")]);
        assert!(q.is_none());
        assert_eq!(report.skipped, skips(&[("tc/2", AGGREGATE)]));
    }

    #[test]
    fn marker_lines() {
        let report = MagicReport {
            rewritten: pairs(&[("tc/2", "bf", 1), ("tc/2", "fb", 0)]),
            skipped: skips(&[("sg/2", OUTPUT), ("w/9", ARITY)]),
        };
        assert_eq!(
            report.lines(),
            [
                "[rules] magic pred=tc/2 adornment=bf sites=1",
                "[rules] magic pred=tc/2 adornment=fb sites=0",
                "[rules] magic skipped pred=sg/2 reason=output",
                "[rules] magic skipped pred=w/9 reason=arity",
            ]
        );
    }

    #[test]
    fn constant_site_seeds_a_fact() {
        let mut rules = tc_right();
        rules.push(rule("out", vec![v("b")], vec![pos("tc", vec![s("a"), v("b")])]));
        let p = program(rules);
        let (text, report) = rewritten(&p, &["out"]);
        assert!(text.contains("__m_tc__bf(\"a\").\nout(b) :- __a_tc__bf(\"a\", b).\n"), "{text}");
        assert_eq!(report.rewritten, pairs(&[("tc/2", "bf", 1)]));
        same_answers(&p, &["out"]);
    }

    #[test]
    fn non_linear_and_same_generation() {
        // tc(x, z) :- tc(x, y), tc(y, z): the second atom's demand reads the first's copy.
        let p = program(vec![
            rule("tc", vec![v("x"), v("y")], vec![edge("x", "y", "CALLS")]),
            rule(
                "tc",
                vec![v("x"), v("z")],
                vec![pos("tc", vec![v("x"), v("y")]), pos("tc", vec![v("y"), v("z")])],
            ),
            out_bound(),
            fact("seed", &["a"]),
        ]);
        let (text, _) = rewritten(&p, &["out"]);
        assert!(text.contains("__m_tc__bf(y) :- __m_tc__bf(x), __a_tc__bf(x, y).\n"), "{text}");
        same_answers(&p, &["out"]);

        // Same generation: up = USES, flat = IMPORTS, down = USES reversed.
        let p = program(vec![
            rule("sg", vec![v("x"), v("y")], vec![edge("x", "y", "IMPORTS")]),
            rule(
                "sg",
                vec![v("x"), v("y")],
                vec![
                    edge("x", "u", "USES"),
                    pos("sg", vec![v("u"), v("w")]),
                    edge("y", "w", "USES"),
                ],
            ),
            rule(
                "out",
                vec![v("b")],
                vec![pos("seed", vec![v("a")]), pos("sg", vec![v("a"), v("b")])],
            ),
            fact("seed", &["a"]),
        ]);
        let (text, report) = rewritten(&p, &["out"]);
        let expected = r#"__a_sg__bf(x, y) :- __m_sg__bf(x), edge(x, y, "IMPORTS").
__a_sg__bf(x, y) :- __m_sg__bf(x), edge(x, u, "USES"), __a_sg__bf(u, w), edge(y, w, "USES").
__m_sg__bf(u) :- __m_sg__bf(x), edge(x, u, "USES").
__m_sg__bf(a) :- seed(a).
out(b) :- seed(a), __a_sg__bf(a, b).
seed("a").
"#;
        assert_eq!(text, expected);
        assert_eq!(report.rewritten, pairs(&[("sg/2", "bf", 1)]));
        same_answers(&p, &["out"]);
    }

    #[test]
    fn mutual_recursion_adorns_the_component() {
        let p = program(vec![
            rule("p", vec![v("x"), v("y")], vec![edge("x", "y", "CALLS")]),
            rule(
                "p",
                vec![v("x"), v("z")],
                vec![pos("q", vec![v("x"), v("y")]), edge("y", "z", "CALLS")],
            ),
            rule(
                "q",
                vec![v("x"), v("z")],
                vec![pos("p", vec![v("x"), v("y")]), edge("y", "z", "USES")],
            ),
            rule(
                "out",
                vec![v("b")],
                vec![pos("seed", vec![v("a")]), pos("p", vec![v("a"), v("b")])],
            ),
            fact("seed", &["a"]),
        ]);
        let (text, report) = rewritten(&p, &["out"]);
        let expected = r#"__a_p__bf(x, y) :- __m_p__bf(x), edge(x, y, "CALLS").
__a_p__bf(x, z) :- __m_p__bf(x), __a_q__bf(x, y), edge(y, z, "CALLS").
__m_q__bf(x) :- __m_p__bf(x).
__a_q__bf(x, z) :- __m_q__bf(x), __a_p__bf(x, y), edge(y, z, "USES").
__m_p__bf(x) :- __m_q__bf(x).
__m_p__bf(a) :- seed(a).
out(b) :- seed(a), __a_p__bf(a, b).
seed("a").
"#;
        assert_eq!(text, expected);
        assert_eq!(report.rewritten, pairs(&[("p/2", "bf", 1), ("q/2", "bf", 0)]));
        same_answers(&p, &["out"]);
    }

    #[test]
    fn demand_flows_down_through_two_components() {
        // reach reads tc; both are recursive candidates. reach is rewritten first, then
        // tc's sites are the ones inside reach's adorned copies.
        let mut rules = tc_left();
        rules.push(rule("reach", vec![v("x"), v("y")], vec![pos("tc", vec![v("x"), v("y")])]));
        rules.push(rule(
            "reach",
            vec![v("x"), v("z")],
            vec![
                pos("reach", vec![v("x"), v("y")]),
                edge("y", "w", "USES"),
                pos("tc", vec![v("w"), v("z")]),
            ],
        ));
        rules.push(rule(
            "out",
            vec![v("b")],
            vec![pos("seed", vec![v("a")]), pos("reach", vec![v("a"), v("b")])],
        ));
        rules.push(fact("seed", &["a"]));
        let p = program(rules);
        let (text, report) = rewritten(&p, &["out"]);
        let expected = r#"__a_tc__bf(x, y) :- __m_tc__bf(x), edge(x, y, "CALLS").
__a_tc__bf(x, z) :- __m_tc__bf(x), __a_tc__bf(x, y), edge(y, z, "CALLS").
__m_tc__bf(x) :- __m_reach__bf(x).
__a_reach__bf(x, y) :- __m_reach__bf(x), __a_tc__bf(x, y).
__m_tc__bf(w) :- __m_reach__bf(x), __a_reach__bf(x, y), edge(y, w, "USES").
__a_reach__bf(x, z) :- __m_reach__bf(x), __a_reach__bf(x, y), edge(y, w, "USES"), __a_tc__bf(w, z).
__m_reach__bf(a) :- seed(a).
out(b) :- seed(a), __a_reach__bf(a, b).
seed("a").
"#;
        assert_eq!(text, expected);
        assert_eq!(report.rewritten, pairs(&[("tc/2", "bf", 2), ("reach/2", "bf", 1)]));
        same_answers(&p, &["out"]);
    }

    #[test]
    fn assignment_binds_and_duplicate_seeds_merge() {
        // `m = "a"` binds m before the site, so the site is bound; two rules with the
        // same seed give one seed rule.
        let mut rules = tc_right();
        rules.push(rule(
            "out",
            vec![v("b")],
            vec![Literal::cmp(CmpOp::Eq, v("m"), s("a")), pos("tc", vec![v("m"), v("b")])],
        ));
        rules.push(rule(
            "out",
            vec![v("b")],
            vec![
                Literal::cmp(CmpOp::Eq, v("m"), s("a")),
                pos("tc", vec![v("m"), v("b")]),
                edge("b", "_n", "USES"),
            ],
        ));
        let p = program(rules);
        let (text, report) = rewritten(&p, &["out"]);
        assert_eq!(text.matches("__m_tc__bf(m) :- m = \"a\".\n").count(), 1, "{text}");
        assert_eq!(report.rewritten, pairs(&[("tc/2", "bf", 2)]));
        same_answers(&p, &["out"]);
    }
}
