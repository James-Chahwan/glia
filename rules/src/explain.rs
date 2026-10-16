//! Evaluation::explain (DA.15): a row's minimal-height derivation at its tier, as a
//! [`Witness`] tree, after Souffle's provenance (Zhao, Subotic, Scholz, TOPLAS 2020).
//! Every derived row records the (rule, height) of the first derivation at its level
//! (DA.14b). A proof is found by searching over those records. Nothing is evaluated
//! again.
//!
//! **The search.** A row of rule R with height H at level L is explained by
//! enumerating the instantiations of R whose body rows sit at levels <= L with heights
//! below H. That is exec's Explain mode over `View { levels: 0..=L, max_height: Some(H)
//! }`. The levelled evaluation inserts a row once, at the first derivation of its
//! level, and never updates it, so the recorded derivation is always among them. The
//! body runs from one of three plans:
//! - [`plan_bound`] with the head's variables bound (no hoist: every literal is
//!   instantiated). Its index permutations are built with `Relation::ensure_index` the
//!   first time they are needed and kept for later explains.
//! - When the evaluation plan hoisted a `reaches` into a ReachMulti, the evaluation's own
//!   base variant, over the seed sets exec kept (DA.14a). Its bindings are filtered to
//!   the row's head. A hoisted reach is explained from the NEAREST seed: the ranked
//!   multi-source path (`Closure::path`) at the reached row's tier, from the kept seeds
//!   of level <= that tier. The seed prefix is then instantiated with the path's first
//!   node bound (its last node for a backward walk).
//! - For an aggregate row, [`plan_aggregate`] (the same plan evaluation ran; when the
//!   two differ the ReachMulti states are filled afresh). The body runs once and is
//!   filtered to the row's group key. The row's support is one Witness per DISTINCT
//!   projected binding (the group columns, then the aggregated variables: DA.8's set
//!   semantics), each its best binding, sorted by rendered values.
//!
//! **The choice.** Of the instantiations, explain keeps the one with the lowest
//! (level, height), then the smallest rendered body: each body literal's row values, in
//! body order, compared as rows sort (`ValueTable::cmp_rendered`). The order never
//! reads a handle or edge order, so the witness survives a checkout that re-salts
//! NodeIds and reverses the edge list. Every instantiation counts against
//! [`ExplainLimits::max_candidates`] for the tuple being searched. Passing the cap is
//! `RulesError::Budget` for that tuple's predicate, never a silent partial choice.
//! The chosen derivation of a tuple is memoised, so a tuple that recurs in one proof
//! is searched once.
//!
//! **Leaves.** An edge-family row (`edge`, `edge_conf`, `edge_cell`) becomes
//! [`Leaf::Edge`] with its stored witness ordinal: the GLOBAL position in
//! `FactSource::edges()` of the first edge with that key at its level (DA.6). DB maps
//! it back to its `&Edge` and reads EVIDENCE there; this crate never parses evidence.
//! `node`, an attribute, `cell` and the registry predicates become [`Leaf::Node`] /
//! [`Leaf::Attr`] / [`Leaf::Cell`] / [`Leaf::Registry`]. A domain or string builtin call
//! becomes [`Leaf::Builtin`]. A `reaches` row becomes [`Leaf::Path`]: a shortest path at
//! the row's tier from its source, parents ranked by rendered key (DA.11). An `scc` row
//! becomes [`Leaf::Path`] of `Closure::cycle`, a shortest cycle that starts and ends at
//! the representative and passes the member. A negated literal is a child Witness with
//! `negated = true`, no support and no tier of its own (Q5: negation adds none). Its
//! values are the literal's arguments, and a wildcard is `Str("_")`. Comparisons and
//! assignments are filters and leave no child. Children follow body order.
//!
//! **Synthetic atoms (DA.12).** A magic atom (`__m_`) is demand bookkeeping, not part of
//! the logical proof, and is dropped with its support. An adorned copy (`__a_`) is
//! spliced: its support moves into its parent in its place. So no witness names a `__`
//! predicate, and the leaf edges are the unrewritten program's.
//!
//! **The leaf cap.** Children are taken depth-first, left to right. Once
//! [`ExplainLimits::max_leaves`] leaves are kept, every child still waiting is dropped
//! and `truncated` is set on the node that lost it. DB reports the count beyond the
//! cap as check does with `MAX_EVIDENCE`. Each leaf counts once, a path included.
//!
//! **No recursion over the proof.** The search, the tree build, [`Witness`]'s Clone,
//! PartialEq, Debug and Drop, and [`Witness::leaf_edges`] all run on explicit stacks,
//! so a 20,000-hop derivation explains on a 2 MiB thread. Only nested hoists recurse,
//! once per nesting level, which the rule's length bounds.
//!
//! **Marker**, on stderr, once per explained row: `[rules] explain pred=<name>/<arity>
//! tier=<fact|derived|heuristic> height=<h> leaves=<n> candidates=<c>
//! truncated=<true|false>`. A candidate budget stop prints `[rules] explain budget
//! pred=<name>/<arity> candidates=<n> cap=<cap>` instead. Single-threaded. No map is
//! iterated.

use std::cmp::Ordering;
use std::fmt;
use std::rc::Rc;

use glia_activation::profile::DomainTables;
use glia_core::{EdgeCategoryId, NodeId};

use crate::aggregate::OutCol;
use crate::analysis::{
    Analysis, CLit, CRule, CTerm, CatSet, ConstVal, EdbPred, PredId, PredKind, closure_set,
};
use crate::closure::{Closure, Hop};
use crate::diag::RulesError;
use crate::eval::{DEFAULT_MAX_ROWS, Evaluation, Row, user_predicate};
use crate::exec::{Binding, Budget, Ctx, LitRow, Memo, Mode, MultiState, run_body};
use crate::ir::{BuiltinKind, Catalog, Program};
use crate::plan::{KeyPart, Step, StratumPlan, plan_aggregate, plan_bound, resolve_indexes};
use crate::relation::{EDB_RULE, Relation, RowMeta};
use crate::source::{Declared, FactSource, NO_ORDINAL};
use crate::value::{Handle, Lookup, Tier, Value, ValueRef, ValueTable};

/// The default leaf cap: check's `MAX_EVIDENCE` (engine/src/check.rs), so an explained
/// violation carries as many edges as check prints.
pub const MAX_LEAVES: usize = 100;

/// An unbound variable in a binding (exec's `FREE`).
const FREE: Handle = Handle::MAX;

/// What one explain may cost. Built with `Default` and field assignment outside the
/// crate; [`Evaluation::explain_limits`] gives the evaluation's own row budget as the
/// candidate cap.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExplainLimits {
    /// Leaves kept before the rest of the tree is dropped ([`MAX_LEAVES`]).
    pub max_leaves: usize,
    /// Instantiations one tuple's search may enumerate ([`DEFAULT_MAX_ROWS`]).
    pub max_candidates: usize,
}

impl Default for ExplainLimits {
    fn default() -> Self {
        ExplainLimits { max_leaves: MAX_LEAVES, max_candidates: DEFAULT_MAX_ROWS }
    }
}

/// One node of a derivation: a tuple and the rule that derived it, a leaf read from
/// the fact source or a builtin, a negated literal, or one binding of an aggregate.
///
/// - A derived tuple has `rule` and its body's witnesses in `support`, in body order.
/// - A leaf has `leaf` and height 0.
/// - A negated literal has `negated` and nothing else.
/// - An aggregate row's `support` holds one binding Witness per distinct projected
///   binding. Each names the aggregate's predicate, holds the projection (the group
///   columns, then the aggregated variables) as `values`, and has the binding's level as
///   `tier` and the highest of its body rows' heights as `height`. Its `support` is its
///   body.
///
/// Clone, PartialEq, Debug and Drop walk the tree on explicit stacks, so a proof
/// thousands of levels deep is no stack risk.
#[non_exhaustive]
pub struct Witness {
    /// `name/arity`: the user predicate (a synthetic copy is never named).
    pub predicate: String,
    pub values: Vec<Value>,
    pub tier: Tier,
    /// The derivation height (0 for a leaf or a negation).
    pub height: u32,
    pub rule: Option<RuleRef>,
    pub negated: bool,
    pub support: Vec<Witness>,
    pub leaf: Option<Leaf>,
    /// Children were dropped here because the leaf cap was reached.
    pub truncated: bool,
}

/// The rule that derived a tuple: its position in the evaluated program and the
/// user's text it came from. With the magic rewrite on, the evaluated program is the
/// rewritten one, while `source` and `line` still name the user's rule (generated
/// rules keep its span).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuleRef {
    pub index: usize,
    pub source: String,
    pub line: u32,
}

/// What a leaf of a derivation read.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Leaf {
    /// An `edge` / `edge_conf` / `edge_cell` row.
    Edge(EdgeHop),
    /// A `reaches` row (a shortest path) or an `scc` row (a cycle through the
    /// representative), hops in edge direction.
    Path(Vec<EdgeHop>),
    /// A `node(id, kind)` row.
    Node { id: NodeId, kind: String },
    /// A domain attribute row.
    Attr { id: NodeId, name: String, value: Value },
    /// A `cell(id, cell_type, text)` row.
    Cell { id: NodeId, cell_type: String },
    /// A domain or string builtin call and its row.
    Builtin { name: String, args: Vec<Value> },
    /// A `kind` / `category` / `carry_category` row.
    Registry,
}

/// One edge of a witness. `ordinal` is its GLOBAL position in `FactSource::edges()`:
/// stable within one graph, not across checkouts, so a renderer prints hops by (from,
/// to, category, evidence), never by ordinal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct EdgeHop {
    pub from: NodeId,
    pub to: NodeId,
    pub category: EdgeCategoryId,
    pub ordinal: usize,
    pub tier: Tier,
}

impl Witness {
    /// Every edge the derivation reads, depth-first and left to right, each ordinal
    /// once (its first occurrence).
    pub fn leaf_edges(&self) -> Vec<EdgeHop> {
        let mut out = Vec::new();
        let mut seen: Lookup<usize, ()> = Lookup::new();
        let mut stack = vec![self];
        while let Some(w) = stack.pop() {
            let hops = match &w.leaf {
                Some(Leaf::Edge(h)) => std::slice::from_ref(h),
                Some(Leaf::Path(hops)) => hops.as_slice(),
                _ => &[],
            };
            for h in hops {
                if !seen.contains_key(&h.ordinal) {
                    seen.insert(h.ordinal, ());
                    out.push(*h);
                }
            }
            stack.extend(w.support.iter().rev());
        }
        out
    }

    /// This node without its support.
    fn shallow(&self) -> Witness {
        Witness {
            predicate: self.predicate.clone(),
            values: self.values.clone(),
            tier: self.tier,
            height: self.height,
            rule: self.rule.clone(),
            negated: self.negated,
            support: Vec::new(),
            leaf: self.leaf.clone(),
            truncated: self.truncated,
        }
    }

    /// A leaf read at `tier`.
    fn leaf(predicate: String, values: Vec<Value>, tier: Tier, leaf: Leaf) -> Witness {
        Witness {
            predicate,
            values,
            tier,
            height: 0,
            rule: None,
            negated: false,
            support: Vec::new(),
            leaf: Some(leaf),
            truncated: false,
        }
    }
}

impl Clone for Witness {
    fn clone(&self) -> Self {
        // Breadth-first, so every node's children come after it: rebuilt back to front.
        let mut nodes: Vec<(&Witness, usize, usize)> = vec![(self, 0, 0)];
        let mut i = 0;
        while i < nodes.len() {
            let w = nodes[i].0;
            let first = nodes.len();
            nodes.extend(w.support.iter().map(|c| (c, 0, 0)));
            (nodes[i].1, nodes[i].2) = (first, w.support.len());
            i += 1;
        }
        let mut done: Vec<Option<Witness>> = (0..nodes.len()).map(|_| None).collect();
        for i in (0..nodes.len()).rev() {
            let (w, first, len) = nodes[i];
            let mut copy = w.shallow();
            copy.support = (first..first + len).filter_map(|c| done[c].take()).collect();
            done[i] = Some(copy);
        }
        match done.into_iter().next() {
            Some(Some(w)) => w,
            _ => self.shallow(),
        }
    }
}

impl PartialEq for Witness {
    fn eq(&self, other: &Self) -> bool {
        let mut stack = vec![(self, other)];
        while let Some((a, b)) = stack.pop() {
            let same = a.predicate == b.predicate
                && a.values == b.values
                && a.tier == b.tier
                && a.height == b.height
                && a.rule == b.rule
                && a.negated == b.negated
                && a.leaf == b.leaf
                && a.truncated == b.truncated
                && a.support.len() == b.support.len();
            if !same {
                return false;
            }
            stack.extend(a.support.iter().zip(&b.support));
        }
        true
    }
}

impl fmt::Debug for Witness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        enum Tok<'a> {
            Node(&'a Witness),
            Text(&'static str),
        }
        let mut stack = vec![Tok::Node(self)];
        while let Some(t) = stack.pop() {
            let w = match t {
                Tok::Text(s) => {
                    f.write_str(s)?;
                    continue;
                }
                Tok::Node(w) => w,
            };
            write!(
                f,
                "Witness {{ predicate: {:?}, values: {:?}, tier: {:?}, height: {}, rule: {:?}, \
                 negated: {}, leaf: {:?}, truncated: {}, support: [",
                w.predicate, w.values, w.tier, w.height, w.rule, w.negated, w.leaf, w.truncated
            )?;
            stack.push(Tok::Text("] }"));
            for (i, c) in w.support.iter().enumerate().rev() {
                stack.push(Tok::Node(c));
                if i > 0 {
                    stack.push(Tok::Text(", "));
                }
            }
        }
        Ok(())
    }
}

impl Drop for Witness {
    fn drop(&mut self) {
        let mut stack = std::mem::take(&mut self.support);
        while let Some(mut w) = stack.pop() {
            stack.append(&mut w.support);
        }
    }
}

impl<'s, S: FactSource + ?Sized> Evaluation<'s, S> {
    /// [`ExplainLimits::default`] with the evaluation's own row budget as the
    /// candidate cap.
    pub fn explain_limits(&self) -> ExplainLimits {
        ExplainLimits { max_leaves: MAX_LEAVES, max_candidates: self.max_rows }
    }

    /// The minimal-height derivation of `row` at its tier (module doc). `row` comes
    /// from this evaluation's [`Evaluation::rows`]: take the rows first, then explain
    /// each. A row of another evaluation is `RulesError::Unknown`. `&mut self`: a
    /// bound plan may read an index the evaluation never built, which is built here
    /// once and kept.
    pub fn explain(&mut self, row: &Row, limits: &ExplainLimits) -> Result<Witness, RulesError> {
        self.explainer(limits).explain(row)
    }

    /// [`Evaluation::explain`] for each row, in order, sharing one closure index and
    /// one derivation memo, so explaining every row of an answer builds the
    /// `reaches` / `scc` index once.
    pub fn explain_all(
        &mut self,
        rows: &[Row],
        limits: &ExplainLimits,
    ) -> Result<Vec<Witness>, RulesError> {
        let mut x = self.explainer(limits);
        rows.iter().map(|r| x.explain(r)).collect()
    }

    fn explainer(&mut self, limits: &ExplainLimits) -> Explainer<'_, S> {
        let outputs = self.outputs();
        let Evaluation {
            src,
            declared,
            program,
            analysis,
            values,
            relations,
            scc,
            tiers,
            memo,
            plans,
            multi,
            ..
        } = self;
        let src: &S = src;
        let tiers: &[Tier] = tiers;
        Explainer {
            src,
            an: analysis,
            declared,
            program,
            values,
            relations,
            scc,
            tiers,
            closure: Closure::new(src, tiers),
            memo,
            plans,
            multi,
            limits: limits.clone(),
            outputs,
            candidates: 0,
            bodies: Lookup::new(),
            chosen: Lookup::new(),
            searched: Vec::new(),
        }
    }
}

/// The `[rules] explain` marker line.
fn explain_line(
    pred: &str,
    tier: Tier,
    height: u32,
    leaves: usize,
    candidates: usize,
    truncated: bool,
) -> String {
    format!(
        "[rules] explain pred={pred} tier={} height={height} leaves={leaves} \
         candidates={candidates} truncated={truncated}",
        tier.name()
    )
}

/// A rule body as explain runs it.
struct Body {
    steps: Vec<Step>,
    /// Per step: its body literal, `usize::MAX` for a comparison or assignment.
    lit_of: Vec<usize>,
    /// The stratum whose kept ReachMulti states the steps read.
    kept: Option<usize>,
    /// For a body run with nothing bound: the head parts a binding must reproduce.
    head: Option<Vec<KeyPart>>,
}

/// One instantiation of a body.
struct Cand {
    vars: Vec<Handle>,
    level: Tier,
    height: u32,
    lits: Vec<LitRow>,
    /// An aggregate binding's projection; empty otherwise.
    proj: Vec<Handle>,
}

/// A child waiting to be placed in the tree.
#[derive(Clone)]
enum Child {
    /// An IDB tuple, expanded when placed; `splice`: an adorned copy, whose support
    /// takes its place.
    Tuple { pred: PredId, row: Vec<Handle>, tier: Tier, meta: RowMeta, splice: bool },
    /// A finished leaf or negation.
    Done(Witness),
    /// An aggregate binding and its body.
    Binding(Witness, Vec<Child>),
}

/// An open node of the tree build: where its children attach, and the children still
/// waiting (reversed, so the next is last).
struct Frame {
    attach: usize,
    pending: Vec<Child>,
}

/// One explain call's (or batch's) view of the evaluation.
struct Explainer<'x, S: FactSource + ?Sized> {
    src: &'x S,
    an: &'x Analysis,
    declared: &'x Declared<'x>,
    program: &'x Program,
    values: &'x mut ValueTable,
    relations: &'x mut Vec<Relation>,
    scc: &'x [(CatSet, Relation)],
    tiers: &'x [Tier],
    closure: Closure<'x, S>,
    memo: &'x mut Memo,
    plans: &'x [StratumPlan],
    multi: &'x mut [Vec<MultiState>],
    limits: ExplainLimits,
    /// For `Unknown`.
    outputs: Vec<String>,
    /// Instantiations enumerated by the row being explained.
    candidates: usize,
    /// By rule index.
    bodies: Lookup<usize, Rc<Body>>,
    /// Each searched tuple's position in `searched`.
    chosen: Lookup<(PredId, Vec<Handle>), usize>,
    searched: Vec<Vec<Child>>,
}

impl<'x, S: FactSource + ?Sized> Explainer<'x, S> {
    fn explain(&mut self, row: &Row) -> Result<Witness, RulesError> {
        self.check_row(row)?;
        let at = &row.at;
        self.candidates = 0;
        let root = self.tuple_witness(at.pred, &at.row, at.level, at.meta)?;
        let first = self.expand(at.pred, &at.row, at.level, at.meta)?;
        let mut arena: Vec<(Witness, Vec<usize>)> = vec![(root, Vec::new())];
        let mut frames = vec![Frame { attach: 0, pending: reversed(first) }];
        let (mut leaves, mut truncated) = (0usize, false);
        while let Some(f) = frames.last_mut() {
            let attach = f.attach;
            if leaves >= self.limits.max_leaves && !f.pending.is_empty() {
                f.pending.clear();
                arena[attach].0.truncated = true;
                truncated = true;
                continue;
            }
            let Some(child) = f.pending.pop() else {
                frames.pop();
                continue;
            };
            match child {
                Child::Tuple { pred, row, tier, meta, splice } => {
                    let kids = self.expand(pred, &row, tier, meta)?;
                    let to = if splice {
                        attach
                    } else {
                        let w = self.tuple_witness(pred, &row, tier, meta)?;
                        place(&mut arena, attach, w)
                    };
                    frames.push(Frame { attach: to, pending: reversed(kids) });
                }
                Child::Done(w) => {
                    leaves += usize::from(w.leaf.is_some());
                    place(&mut arena, attach, w);
                }
                Child::Binding(w, kids) => {
                    let to = place(&mut arena, attach, w);
                    frames.push(Frame { attach: to, pending: reversed(kids) });
                }
            }
        }
        // Children are placed after their parent: assemble back to front.
        let n = arena.len();
        let mut done: Vec<Option<Witness>> = (0..n).map(|_| None).collect();
        for (i, (mut w, kids)) in arena.into_iter().enumerate().rev() {
            w.support = kids.iter().filter_map(|&c| done[c].take()).collect();
            done[i] = Some(w);
        }
        let Some(Some(root)) = done.into_iter().next() else {
            return Err(self.internal(at.pred, "the witness tree lost its root".into()));
        };
        let sig = self.signature(at.pred);
        let line = explain_line(&sig, at.level, at.meta.height, leaves, self.candidates, truncated);
        eprintln!("{line}");
        Ok(root)
    }

    /// `row` is a row this evaluation holds: a user IDB predicate, its relation's
    /// level and provenance, and its values interned here under its handles.
    fn check_row(&self, row: &Row) -> Result<(), RulesError> {
        let at = &row.at;
        let info = self.an.preds.get(at.pred as usize);
        let user = info.is_some_and(|i| i.kind == PredKind::Idb && !i.synthetic);
        let held = self.relations.get(at.pred as usize).and_then(|r| r.contains(&at.row));
        let same = user
            && held.is_some_and(|(t, m, _)| t == at.level && m == at.meta)
            && at.row.len() == row.values.len()
            && at
                .row
                .iter()
                .zip(&row.values)
                .all(|(h, v)| self.values.find(v.as_ref()) == Some(*h));
        if same {
            return Ok(());
        }
        let predicate = match info {
            Some(i) => i.signature(),
            None => format!("#{}/{}", at.pred, row.values.len()),
        };
        Err(RulesError::Unknown { predicate, known: self.outputs.clone() })
    }

    /// `p` as `name/arity`, a synthetic copy by its user predicate.
    fn signature(&self, p: PredId) -> String {
        user_predicate(self.an, p).map_or_else(|| format!("#{p}"), |(n, a)| format!("{n}/{a}"))
    }

    fn internal(&self, p: PredId, message: String) -> RulesError {
        RulesError::Type { predicate: self.signature(p), message: format!("explain: {message}") }
    }

    fn owned(&self, row: &[Handle]) -> Vec<Value> {
        row.iter().map(|h| self.values.get(*h).to_owned()).collect()
    }

    fn rule_ref(&self, rule: &CRule) -> RuleRef {
        RuleRef {
            index: rule.index,
            source: self.program.source_name(rule.span.source).to_string(),
            line: rule.span.line,
        }
    }

    /// The rule a tuple's provenance names: its recorded rule, or for a loaded
    /// (aggregate) row its predicate's only rule.
    fn rule_of(&self, pred: PredId, meta: RowMeta) -> Result<&'x CRule, RulesError> {
        let an: &'x Analysis = self.an;
        let found = if meta.rule == EDB_RULE {
            an.rules.iter().find(|r| r.head == pred)
        } else {
            an.rules.iter().find(|r| r.index == usize::from(meta.rule))
        };
        match found.filter(|r| r.head == pred) {
            Some(r) => Ok(r),
            None => Err(self.internal(pred, format!("no rule {} derives the row", meta.rule))),
        }
    }

    fn tuple_witness(
        &self,
        pred: PredId,
        row: &[Handle],
        tier: Tier,
        meta: RowMeta,
    ) -> Result<Witness, RulesError> {
        let rule = self.rule_of(pred, meta)?;
        Ok(Witness {
            predicate: self.signature(pred),
            values: self.owned(row),
            tier,
            height: meta.height,
            rule: Some(self.rule_ref(rule)),
            negated: false,
            support: Vec::new(),
            leaf: None,
            truncated: false,
        })
    }

    /// A tuple's chosen children, searched once per tuple.
    fn expand(
        &mut self,
        pred: PredId,
        row: &[Handle],
        level: Tier,
        meta: RowMeta,
    ) -> Result<Vec<Child>, RulesError> {
        let key = (pred, row.to_vec());
        if let Some(&i) = self.chosen.get(&key) {
            return Ok(self.searched.get(i).cloned().unwrap_or_default());
        }
        let rule = self.rule_of(pred, meta)?;
        let kids = if rule.aggs.is_empty() {
            self.search(rule, row, level, meta)?
        } else {
            self.aggregate_support(rule, row, level, meta)?
        };
        let slot = self.chosen.len();
        self.chosen.insert(key, slot);
        self.searched.push(kids.clone());
        Ok(kids)
    }

    /// The best instantiation of `rule` deriving `row` (module doc), as children.
    fn search(
        &mut self,
        rule: &'x CRule,
        row: &[Handle],
        level: Tier,
        meta: RowMeta,
    ) -> Result<Vec<Child>, RulesError> {
        let body = self.body(rule)?;
        let mut pairs: Vec<(u16, Handle)> = Vec::new();
        if body.head.is_none() {
            for (t, &h) in rule.head_terms.iter().zip(row) {
                let CTerm::Var(v) = t else { continue };
                match pairs.iter().find(|(w, _)| w == v) {
                    Some(&(_, g)) if g != h => {
                        let msg = "a repeated head variable holds two values".to_string();
                        return Err(self.internal(rule.head, msg));
                    }
                    Some(_) => {}
                    None => pairs.push((*v, h)),
                }
            }
        }
        let head: &[KeyPart] = body.head.as_deref().unwrap_or(&[]);
        let consts: Vec<Option<Handle>> = head
            .iter()
            .map(|p| match p {
                KeyPart::Var(_) => None,
                KeyPart::Const(c) => self.const_handle(c),
            })
            .collect();
        let accept = |b: &Binding<'_>| -> Option<Vec<Handle>> {
            let fits = head.iter().zip(&consts).zip(row).all(|((p, c), want)| match p {
                KeyPart::Var(v) => b.vars.get(usize::from(*v)) == Some(want),
                KeyPart::Const(_) => *c == Some(*want),
            });
            fits.then(Vec::new)
        };
        let mode = Mode::Explain { level, max_height: meta.height, bound: &pairs };
        let mut fresh = Vec::new();
        let cands = self.collect(&body.steps, body.kept, &mut fresh, mode, rule, &accept, true)?;
        let best = self.pick(&cands, &body.lit_of, rule, level, meta.height)?;
        let kids = self.children(
            rule,
            &body.steps,
            &body.lit_of,
            best,
            body.kept,
            &mut fresh,
            meta.height,
        )?;
        Ok(kids)
    }

    /// The plan `rule`'s rows are explained with, built once per rule.
    fn body(&mut self, rule: &'x CRule) -> Result<Rc<Body>, RulesError> {
        if let Some(b) = self.bodies.get(&rule.index) {
            return Ok(Rc::clone(b));
        }
        let (an, plans, declared) = (self.an, self.plans, self.declared);
        let catalog = &declared.catalog;
        let k = an.preds.get(rule.head as usize).map_or(0, |i| i.stratum);
        let base = plans
            .get(k)
            .and_then(|p| p.rules.iter().find(|r| r.rule == rule.index))
            .and_then(|r| r.variants.iter().find(|v| v.delta.is_none()));
        let hoisted = base.filter(|v| v.steps.iter().any(|s| matches!(s, Step::ReachMulti { .. })));
        let body = match hoisted {
            Some(v) => Body {
                lit_of: map_steps(rule, &v.steps, an, catalog),
                steps: v.steps.clone(),
                kept: Some(k),
                head: Some(v.head.clone()),
            },
            None => {
                let mut mask = vec![false; usize::from(rule.nvars)];
                for t in &rule.head_terms {
                    if let CTerm::Var(v) = t
                        && let Some(m) = mask.get_mut(usize::from(*v))
                    {
                        *m = true;
                    }
                }
                let (mut steps, indexes) = plan_bound(an, catalog, rule.index, &mask);
                let map = self.ensure(&indexes)?;
                resolve_indexes(&mut steps, &map);
                Body { lit_of: map_steps(rule, &steps, an, catalog), steps, kept: None, head: None }
            }
        };
        let body = Rc::new(body);
        self.bodies.insert(rule.index, Rc::clone(&body));
        Ok(body)
    }

    /// Each `(predicate, permutation)`'s index number, built when missing.
    fn ensure(&mut self, indexes: &[(PredId, Vec<u8>)]) -> Result<Vec<usize>, RulesError> {
        let mut map = Vec::with_capacity(indexes.len());
        for (p, perm) in indexes {
            match self.relations.get_mut(*p as usize).and_then(|r| r.ensure_index(perm)) {
                Some(i) => map.push(i),
                None => {
                    let sig = self.signature(*p);
                    let message = format!(
                        "`{sig}` is too wide to index: a relation key holds at most 256 columns"
                    );
                    return Err(RulesError::Type { predicate: sig, message });
                }
            }
        }
        Ok(map)
    }

    /// A constant's handle, when some row holds it.
    fn const_handle(&self, c: &ConstVal) -> Option<Handle> {
        match c {
            ConstVal::Str(s) => self.values.find(ValueRef::Str(s)),
            ConstVal::Int(i) => self.values.find(ValueRef::Int(*i)),
            ConstVal::CatSet(set) => {
                self.values.find(ValueRef::Str(&set_text(set, self.src.tables())))
            }
        }
    }

    /// Run `steps` over the evaluation's relations (and the kept ReachMulti states of
    /// stratum `kept`, else `fresh`, which exec fills when a hoist needs it).
    fn run(
        &mut self,
        steps: &[Step],
        kept: Option<usize>,
        fresh: &mut Vec<MultiState>,
        mode: Mode<'_>,
        emit: &mut dyn FnMut(&Binding<'_>) -> Result<(), RulesError>,
    ) -> Result<(), RulesError> {
        let mut budget = Budget::new(usize::MAX);
        let multi = match kept {
            Some(k) if k < self.multi.len() => &mut self.multi[k],
            _ => fresh,
        };
        let mut ctx = Ctx {
            src: self.src,
            values: &mut *self.values,
            rels: &self.relations[..],
            scc: self.scc,
            closure: &mut self.closure,
            builtins: &self.declared.impls,
            decls: &self.declared.catalog.builtins,
            memo: &mut *self.memo,
            multi,
            budget: &mut budget,
        };
        run_body(&mut ctx, steps, mode, emit)
    }

    /// The instantiations `accept` keeps (it returns the projection to keep with
    /// one), each counted against the candidate cap for `rule`'s head. With `prune`,
    /// only those of the lowest (level, height) seen are kept.
    #[allow(clippy::too_many_arguments)] // one body run: its plan, its multi states, its view and filter
    fn collect(
        &mut self,
        steps: &[Step],
        kept: Option<usize>,
        fresh: &mut Vec<MultiState>,
        mode: Mode<'_>,
        rule: &CRule,
        accept: &dyn Fn(&Binding<'_>) -> Option<Vec<Handle>>,
        prune: bool,
    ) -> Result<Vec<Cand>, RulesError> {
        let cap = self.limits.max_candidates;
        let predicate = self.signature(rule.head);
        let source = self.program.source_name(rule.span.source).to_string();
        let line = rule.span.line;
        let (mut count, mut over) = (0usize, false);
        let mut best: Option<(Tier, u32)> = None;
        let mut cands: Vec<Cand> = Vec::new();
        let ran = self.run(steps, kept, fresh, mode, &mut |b| {
            let Some(proj) = accept(b) else { return Ok(()) };
            count += 1;
            if count > cap {
                over = true;
                let (predicate, source) = (predicate.clone(), source.clone());
                return Err(RulesError::Budget { predicate, source, line, rows: count, cap });
            }
            let key = (b.level, b.height);
            if prune {
                match best {
                    Some(k) if key > k => return Ok(()),
                    Some(k) if key < k => cands.clear(),
                    _ => {}
                }
                best = Some(best.map_or(key, |k| k.min(key)));
            }
            let lits = b.lits.to_vec();
            cands.push(Cand {
                vars: b.vars.to_vec(),
                level: b.level,
                height: b.height,
                lits,
                proj,
            });
            Ok(())
        });
        self.candidates += count;
        if over {
            eprintln!("[rules] explain budget pred={predicate} candidates={count} cap={cap}");
        }
        ran?;
        Ok(cands)
    }

    /// The best of `cands`: the lowest (level, height), then the smallest rendered
    /// body. None at all breaks the search invariant, reported for `rule`.
    fn pick<'c>(
        &mut self,
        cands: &'c [Cand],
        lit_of: &[usize],
        rule: &CRule,
        level: Tier,
        height: u32,
    ) -> Result<&'c Cand, RulesError> {
        let Some(mut best) = cands.first() else {
            let at = self.rule_ref(rule);
            let message = format!(
                "no instantiation of the rule at {}:{} below height {height} at level {}",
                at.source,
                at.line,
                level.name()
            );
            return Err(self.internal(rule.head, message));
        };
        for c in &cands[1..] {
            let o = c.level.cmp(&best.level).then(c.height.cmp(&best.height));
            let o = if o.is_eq() { self.cmp_body(&c.lits, &best.lits, lit_of) } else { o };
            if o.is_lt() {
                best = c;
            }
        }
        Ok(best)
    }

    /// Two instantiations of one plan by their rows' rendered values, in body order.
    fn cmp_body(&mut self, a: &[LitRow], b: &[LitRow], lit_of: &[usize]) -> Ordering {
        let mut order: Vec<usize> = (0..a.len().min(b.len())).collect();
        order.sort_by_key(|&j| (lit_of.get(a[j].step).copied().unwrap_or(usize::MAX), j));
        for j in order {
            let o = self.cmp_handles(&a[j].row, &b[j].row);
            if o.is_ne() {
                return o;
            }
        }
        a.len().cmp(&b.len())
    }

    /// Two rows as output sorts them: column by column, `ValueTable::cmp_rendered`.
    fn cmp_handles(&mut self, a: &[Handle], b: &[Handle]) -> Ordering {
        let src = self.src;
        for (x, y) in a.iter().zip(b) {
            let o = self.values.cmp_rendered(*x, *y, &|id| src.render_node(id));
            if o.is_ne() {
                return o;
            }
        }
        a.len().cmp(&b.len())
    }

    /// The children of instantiation `cand` of `steps`, in body order.
    #[allow(clippy::too_many_arguments)] // one instantiation: its rule, plan, binding and multi states
    fn children(
        &mut self,
        rule: &'x CRule,
        steps: &[Step],
        lit_of: &[usize],
        cand: &Cand,
        kept: Option<usize>,
        fresh: &mut Vec<MultiState>,
        max_height: u32,
    ) -> Result<Vec<Child>, RulesError> {
        let mut out = self.children_at(rule, steps, lit_of, cand, kept, fresh, max_height)?;
        out.sort_by_key(|(li, _)| *li);
        Ok(out.into_iter().map(|(_, c)| c).collect())
    }

    /// [`Self::children`], each with its body literal's index, unsorted.
    #[allow(clippy::too_many_arguments)] // as children
    fn children_at(
        &mut self,
        rule: &'x CRule,
        steps: &[Step],
        lit_of: &[usize],
        cand: &Cand,
        kept: Option<usize>,
        fresh: &mut Vec<MultiState>,
        max_height: u32,
    ) -> Result<Vec<(usize, Child)>, RulesError> {
        let mut out = Vec::new();
        for (k, step) in steps.iter().enumerate() {
            let li = lit_of.get(k).copied().unwrap_or(usize::MAX);
            let lit = rule.body.get(li);
            let unmatched = || format!("step {k} of rule {} matches no body literal", rule.index);
            match step {
                Step::Cmp { .. } | Step::Assign { .. } => {}
                Step::Neg { .. } | Step::NegCall { .. } | Step::NegReach { .. } => {
                    let Some(CLit::Neg(p, terms, _)) = lit else {
                        return Err(self.internal(rule.head, unmatched()));
                    };
                    let values = terms.iter().map(|t| self.term_value(t, &cand.vars)).collect();
                    let w = Witness {
                        predicate: self.signature(*p),
                        values,
                        tier: Tier::Fact,
                        height: 0,
                        rule: None,
                        negated: true,
                        support: Vec::new(),
                        leaf: None,
                        truncated: false,
                    };
                    out.push((li, Child::Done(w)));
                }
                Step::ReachMulti { .. } => {
                    let more = self.hoisted(rule, step, (k, li), cand, kept, fresh, max_height)?;
                    out.extend(more);
                }
                Step::Scan { .. }
                | Step::Lookup { .. }
                | Step::Call { .. }
                | Step::Reach { .. } => {
                    let (Some(CLit::Pos(p, terms, _)), Some(row)) =
                        (lit, cand.lits.iter().find(|l| l.step == k))
                    else {
                        return Err(self.internal(rule.head, unmatched()));
                    };
                    if let Some(c) = self.positive(*p, terms, step, row, &cand.vars)? {
                        out.push((li, c));
                    }
                }
            }
        }
        Ok(out)
    }

    /// The child of a positive literal's row: an IDB tuple (none for a magic atom),
    /// an EDB leaf or a builtin leaf.
    fn positive(
        &mut self,
        p: PredId,
        terms: &[CTerm],
        step: &Step,
        lit: &LitRow,
        vars: &[Handle],
    ) -> Result<Option<Child>, RulesError> {
        let Some(info) = self.an.preds.get(p as usize) else {
            return Err(self.internal(p, "a row of an unknown predicate".into()));
        };
        match info.kind {
            PredKind::Idb => {
                if info.synthetic && info.name.starts_with("__m_") {
                    return Ok(None);
                }
                let (row, tier, meta) = (lit.row.clone(), lit.tier, lit.meta);
                Ok(Some(Child::Tuple { pred: p, row, tier, meta, splice: info.synthetic }))
            }
            PredKind::Edb(e) => Ok(Some(Child::Done(self.edb_leaf(p, e, lit)?))),
            PredKind::Builtin(b) => {
                Ok(Some(Child::Done(self.builtin_leaf(p, b, terms, step, lit, vars)?)))
            }
        }
    }

    /// An EDB row's leaf.
    fn edb_leaf(&self, p: PredId, e: EdbPred, lit: &LitRow) -> Result<Witness, RulesError> {
        let vals = self.owned(&lit.row);
        let node = |i: usize| match vals.get(i) {
            Some(Value::Node(n)) => Ok(*n),
            _ => Err(self.internal(p, format!("column {i} of the row is no node"))),
        };
        let text = |i: usize| match vals.get(i) {
            Some(Value::Str(s)) => s.clone(),
            Some(Value::Int(n)) => n.to_string(),
            Some(Value::Node(n)) => self.src.render_node(*n),
            None => String::new(),
        };
        let leaf = match e {
            EdbPred::Edge | EdbPred::EdgeConf | EdbPred::EdgeCell => {
                let (from, to) = (node(0)?, node(1)?);
                let category = self.category_of(&text(2), lit.ordinal);
                let ordinal = if lit.ordinal == NO_ORDINAL {
                    self.first_edge(from, to, category, lit.tier)
                } else {
                    Some(lit.ordinal as usize)
                };
                let Some(ordinal) = ordinal else {
                    return Err(self.internal(p, "an edge row with no edge behind it".into()));
                };
                Leaf::Edge(EdgeHop { from, to, category, ordinal, tier: lit.tier })
            }
            EdbPred::Node => Leaf::Node { id: node(0)?, kind: text(1) },
            EdbPred::Cell => Leaf::Cell { id: node(0)?, cell_type: text(1) },
            EdbPred::Attr(_) => {
                let name =
                    self.an.preds.get(p as usize).map_or_else(String::new, |i| i.name.clone());
                let value = vals.get(1).cloned().unwrap_or(Value::Str(String::new()));
                Leaf::Attr { id: node(0)?, name, value }
            }
            EdbPred::Kind | EdbPred::Category | EdbPred::CarryCategory => Leaf::Registry,
        };
        Ok(Witness::leaf(self.signature(p), vals, lit.tier, leaf))
    }

    /// The category an edge row names: by registry name (`#<id>` when unregistered),
    /// else read off the edge at `ordinal`.
    fn category_of(&self, name: &str, ordinal: u32) -> EdgeCategoryId {
        let reg = &self.src.tables().registries;
        if let Some((id, _)) = reg.edge_categories.iter().find(|(_, n)| *n == name) {
            return *id;
        }
        if let Some(id) = name.strip_prefix('#').and_then(|n| n.parse::<u32>().ok()) {
            return EdgeCategoryId(id);
        }
        self.src.edges().nth(ordinal as usize).map_or(EdgeCategoryId(u32::MAX), |e| e.category)
    }

    /// The first edge with this key at `tier`, in global order.
    fn first_edge(
        &self,
        from: NodeId,
        to: NodeId,
        cat: EdgeCategoryId,
        tier: Tier,
    ) -> Option<usize> {
        self.src.edges().enumerate().position(|(i, e)| {
            e.from == from && e.to == to && e.category == cat && self.tiers.get(i) == Some(&tier)
        })
    }

    /// A builtin row's leaf: a `reaches` path, an `scc` cycle, or the call itself.
    fn builtin_leaf(
        &mut self,
        p: PredId,
        b: usize,
        terms: &[CTerm],
        step: &Step,
        lit: &LitRow,
        vars: &[Handle],
    ) -> Result<Witness, RulesError> {
        let mut values = self.owned(&lit.row);
        values.extend(terms.iter().skip(lit.row.len()).map(|t| self.term_value(t, vars)));
        let ends = match (values.first(), values.get(1)) {
            (Some(Value::Node(x)), Some(Value::Node(y))) => Some((*x, *y)),
            _ => None,
        };
        let decls = &self.declared.catalog.builtins;
        let scc = decls.get(b).is_some_and(|d| d.kind == BuiltinKind::Scc);
        let leaf = match (step, ends) {
            (Step::Reach { cats, .. }, Some((from, to))) => {
                let Some(hops) = self.closure.path(cats, &[from], to, lit.tier, false) else {
                    return Err(self.internal(p, "a reaches row with no path behind it".into()));
                };
                Leaf::Path(hops.iter().map(edge_hop).collect())
            }
            (Step::Call { cats: Some(cats), .. }, Some((x, rep))) if scc => {
                let hops = self.closure.cycle(cats, x, rep, lit.tier);
                if hops.is_empty() {
                    return Err(self.internal(p, "an scc row with no cycle behind it".into()));
                }
                Leaf::Path(hops.iter().map(edge_hop).collect())
            }
            _ => {
                let name = decls.get(b).map_or_else(String::new, |d| d.name.clone());
                Leaf::Builtin { name, args: self.owned(&lit.row) }
            }
        };
        Ok(Witness::leaf(self.signature(p), values, lit.tier, leaf))
    }

    /// A hoisted reach's children: the reaches literal, explained by the path from
    /// the nearest kept seed at the reached row's tier, then the seed prefix
    /// instantiated with that seed bound.
    #[allow(clippy::too_many_arguments)] // as children, plus the step's position and literal
    fn hoisted(
        &mut self,
        rule: &'x CRule,
        step: &Step,
        (k, li): (usize, usize),
        cand: &Cand,
        kept: Option<usize>,
        fresh: &mut Vec<MultiState>,
        max_height: u32,
    ) -> Result<Vec<(usize, Child)>, RulesError> {
        let Step::ReachMulti { slot, cats, seed, seed_var, backward, bind, .. } = step else {
            return Ok(Vec::new());
        };
        let head = rule.head;
        let (Some(lit), Some(CLit::Pos(p, terms, _))) =
            (cand.lits.iter().find(|l| l.step == k), rule.body.get(li))
        else {
            return Err(self.internal(head, "a hoisted reach matches no body literal".into()));
        };
        let (reached_h, t) = (lit.row.first().copied().unwrap_or(FREE), lit.tier);
        let ValueRef::Node(reached) = self.values.get(reached_h) else {
            return Err(self.internal(head, "a hoisted reach reached no node".into()));
        };
        let states: &[MultiState] = match kept.and_then(|s| self.multi.get(s)) {
            Some(m) => m,
            None => fresh,
        };
        let seeds: Vec<NodeId> = states
            .get(*slot)
            .map(|m| m.seeds.iter().filter(|(_, l)| *l <= t).map(|(id, _)| *id).collect())
            .unwrap_or_default();
        let hops = self.closure.path(cats, &seeds, reached, t, *backward).unwrap_or_default();
        let nearest =
            if *backward { hops.last().map(|h| h.to) } else { hops.first().map(|h| h.from) };
        let Some(s) = nearest.and_then(|n| self.values.find(ValueRef::Node(n))) else {
            return Err(self.internal(head, "a hoisted reach with no path from a seed".into()));
        };
        // The outer binding's variables stop at what its steps bind: a seed's are inside.
        let mut vars = cand.vars.clone();
        for (v, h) in [(*seed_var, s), (*bind, reached_h)] {
            let v = usize::from(v);
            if vars.len() <= v {
                vars.resize(v + 1, FREE);
            }
            vars[v] = h;
        }
        let values = terms.iter().map(|t| self.term_value(t, &vars)).collect();
        let path = Leaf::Path(hops.iter().map(edge_hop).collect());
        let mut out = vec![(li, Child::Done(Witness::leaf(self.signature(*p), values, t, path)))];
        let seed_lit_of = map_steps(rule, seed, self.an, &self.declared.catalog);
        let sv = usize::from(*seed_var);
        let accept = |b: &Binding<'_>| (b.vars.get(sv) == Some(&s)).then(Vec::new);
        let mode = Mode::Explain { level: t, max_height, bound: &[] };
        let cands = self.collect(seed, kept, fresh, mode, rule, &accept, true)?;
        let best = self.pick(&cands, &seed_lit_of, rule, t, max_height)?;
        out.extend(self.children_at(rule, seed, &seed_lit_of, best, kept, fresh, max_height)?);
        Ok(out)
    }

    /// An aggregate row's support (module doc): one binding Witness per distinct
    /// projection in its group, sorted by rendered values.
    fn aggregate_support(
        &mut self,
        rule: &'x CRule,
        row: &[Handle],
        level: Tier,
        meta: RowMeta,
    ) -> Result<Vec<Child>, RulesError> {
        let (an, plans, declared) = (self.an, self.plans, self.declared);
        let k = an.preds.get(rule.head as usize).map_or(0, |i| i.stratum);
        // The sizes evaluation planned the stratum with: the EDB and lower strata.
        let planned = {
            let rels: &[Relation] = self.relations;
            let size = |p: PredId| -> Option<usize> {
                let info = an.preds.get(p as usize)?;
                let complete = match info.kind {
                    PredKind::Edb(_) => true,
                    PredKind::Idb => info.stratum < k,
                    PredKind::Builtin(_) => false,
                };
                complete.then(|| rels.get(p as usize).map_or(0, Relation::len))
            };
            plan_aggregate(an, &declared.catalog, rule.index, &size)
        };
        let Some(mut plan) = planned else {
            return Err(
                self.internal(rule.head, "an aggregate row of a rule with no aggregate".into())
            );
        };
        let map = self.ensure(&plan.indexes)?;
        resolve_indexes(&mut plan.steps, &map);
        let ran = plans.get(k).and_then(|p| p.rules.first()).and_then(|r| r.variants.first());
        let kept = ran.is_some_and(|v| v.steps == plan.steps).then_some(k);
        let mut fresh = vec![MultiState::default(); plan.multi_slots];
        let group = plan.spec.group;
        let mut key = vec![FREE; group];
        for (col, h) in plan.spec.out_order.iter().zip(row) {
            if let OutCol::Group(i) = col
                && let Some(slot) = key.get_mut(*i)
            {
                *slot = *h;
            }
        }
        let consts: Vec<Option<Handle>> = plan
            .project
            .iter()
            .map(|p| match p {
                KeyPart::Var(_) => None,
                KeyPart::Const(c) => self.const_handle(c),
            })
            .collect();
        let project = &plan.project;
        let accept = |b: &Binding<'_>| -> Option<Vec<Handle>> {
            let proj: Vec<Handle> = project
                .iter()
                .zip(&consts)
                .map(|(p, c)| match p {
                    KeyPart::Var(v) => b.vars.get(usize::from(*v)).copied().unwrap_or(FREE),
                    KeyPart::Const(_) => c.unwrap_or(FREE),
                })
                .collect();
            (proj.get(..group) == Some(&key[..])).then_some(proj)
        };
        let lit_of = map_steps(rule, &plan.steps, an, &declared.catalog);
        let mode = Mode::Explain { level, max_height: meta.height, bound: &[] };
        let mut cands = self.collect(&plan.steps, kept, &mut fresh, mode, rule, &accept, false)?;
        // Group by projection (handle order only gathers a group), then each group's
        // best binding, then the groups by rendered projection.
        cands.sort_by(|a, b| a.proj.cmp(&b.proj));
        let mut chosen: Vec<&Cand> = Vec::new();
        let mut i = 0;
        while i < cands.len() {
            let mut j = i + 1;
            while j < cands.len() && cands[j].proj == cands[i].proj {
                j += 1;
            }
            chosen.push(self.pick(&cands[i..j], &lit_of, rule, level, meta.height)?);
            i = j;
        }
        chosen.sort_by(|a, b| self.cmp_handles(&a.proj, &b.proj));
        let (sig, rr) = (self.signature(rule.head), self.rule_ref(rule));
        let mut out = Vec::with_capacity(chosen.len());
        for cand in chosen {
            let kids =
                self.children(rule, &plan.steps, &lit_of, cand, kept, &mut fresh, meta.height)?;
            let w = Witness {
                predicate: sig.clone(),
                values: self.owned(&cand.proj),
                tier: cand.level,
                height: cand.height,
                rule: Some(rr.clone()),
                negated: false,
                support: Vec::new(),
                leaf: None,
                truncated: false,
            };
            out.push(Child::Binding(w, kids));
        }
        Ok(out)
    }

    /// A literal argument's value under `vars`: an unbound variable or `_` is
    /// `Str("_")`; a category set reads as its names sorted and `|`-joined (`*`: all).
    fn term_value(&self, t: &CTerm, vars: &[Handle]) -> Value {
        match t {
            CTerm::Var(v) => match vars.get(usize::from(*v)).filter(|h| **h != FREE) {
                Some(h) => self.values.get(*h).to_owned(),
                None => Value::Str("_".to_string()),
            },
            CTerm::Wild => Value::Str("_".to_string()),
            CTerm::Const(ConstVal::Str(s)) => Value::Str(s.clone()),
            CTerm::Const(ConstVal::Int(i)) => Value::Int(*i),
            CTerm::Const(ConstVal::CatSet(set)) => Value::Str(set_text(set, self.src.tables())),
        }
    }
}

/// Put `w` under `parent`; its position.
fn place(arena: &mut Vec<(Witness, Vec<usize>)>, parent: usize, w: Witness) -> usize {
    let id = arena.len();
    arena.push((w, Vec::new()));
    if let Some((_, kids)) = arena.get_mut(parent) {
        kids.push(id);
    }
    id
}

fn reversed(mut kids: Vec<Child>) -> Vec<Child> {
    kids.reverse();
    kids
}

fn edge_hop(h: &Hop) -> EdgeHop {
    EdgeHop {
        from: h.from,
        to: h.to,
        category: h.category,
        ordinal: h.ordinal as usize,
        tier: h.tier,
    }
}

/// A category set as a builtin column reads it: names sorted and `|`-joined, `*` for
/// every category (exec's form).
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

/// Each step's body literal (`usize::MAX`: none, a comparison or an assignment),
/// matched by content: the literal the planner built the step from, the first unused
/// one in body order (two literals that match one step are the same literal twice).
fn map_steps(rule: &CRule, steps: &[Step], an: &Analysis, catalog: &Catalog) -> Vec<usize> {
    let mut used = vec![false; rule.body.len()];
    steps
        .iter()
        .map(|s| {
            let found =
                (0..rule.body.len()).find(|&i| !used[i] && step_is(s, &rule.body[i], an, catalog));
            if let Some(i) = found {
                used[i] = true;
            }
            found.unwrap_or(usize::MAX)
        })
        .collect()
}

/// Was `step` planned from `lit`? Its columns, rebuilt from the key, binds and checks,
/// equal the literal's terms.
fn step_is(step: &Step, lit: &CLit, an: &Analysis, catalog: &Catalog) -> bool {
    let builtin = |p: &PredId| match an.preds.get(*p as usize).map(|i| i.kind) {
        Some(PredKind::Builtin(b)) => Some(b),
        _ => None,
    };
    let walks = |p: &PredId, terms: &[CTerm], cats: &CatSet, depth: &Option<u32>| {
        let Some(b) = builtin(p) else { return false };
        let reaches = catalog.builtins.get(b).is_some_and(|d| d.kind == BuiltinKind::Reaches);
        reaches
            && closure_set(catalog, b, terms).as_ref() == Some(cats)
            && depth_of(terms) == *depth
    };
    match (step, lit) {
        (Step::Scan { pred, binds, checks, .. }, CLit::Pos(p, terms, _)) => {
            p == pred && columns_are(terms, &columns(terms.len(), &[], &[], binds, checks))
        }
        (Step::Lookup { pred, perm, key, binds, checks, .. }, CLit::Pos(p, terms, _)) => {
            p == pred && columns_are(terms, &columns(terms.len(), perm, key, binds, checks))
        }
        (Step::Neg { pred, perm, key, .. }, CLit::Neg(p, terms, _)) => {
            p == pred && columns_are(terms, &columns(terms.len(), perm, key, &[], &[]))
        }
        (Step::Call { builtin: b, args, binds, checks, .. }, CLit::Pos(p, terms, _)) => {
            builtin(p) == Some(*b) && columns_are(terms, &with_free(args.clone(), binds, checks))
        }
        (Step::NegCall { builtin: b, args, .. }, CLit::Neg(p, terms, _)) => {
            builtin(p) == Some(*b) && columns_are(terms, args)
        }
        (Step::Reach { cats, depth, from, to, binds, checks, .. }, CLit::Pos(p, terms, _)) => {
            let ends = with_free(vec![from.clone(), to.clone()], binds, checks);
            walks(p, terms, cats, depth) && ends_are(terms, &ends)
        }
        (Step::NegReach { cats, depth, from, to }, CLit::Neg(p, terms, _)) => {
            walks(p, terms, cats, depth) && ends_are(terms, &[from.clone(), to.clone()])
        }
        (
            Step::ReachMulti { cats, depth, seed_var, bind, backward, .. },
            CLit::Pos(p, terms, _),
        ) => {
            let (s, b) = (Some(KeyPart::Var(*seed_var)), Some(KeyPart::Var(*bind)));
            let ends = if *backward { [b, s] } else { [s, b] };
            walks(p, terms, cats, depth) && ends_are(terms, &ends)
        }
        _ => false,
    }
}

/// A relation step's columns: the key at its permutation's leading columns, then the
/// binds and checks.
fn columns(
    n: usize,
    perm: &[u8],
    key: &[KeyPart],
    binds: &[(u8, u16)],
    checks: &[(u8, KeyPart)],
) -> Vec<Option<KeyPart>> {
    let mut cols = vec![None; n];
    for (part, c) in key.iter().zip(perm) {
        if let Some(slot) = cols.get_mut(usize::from(*c)) {
            *slot = Some(part.clone());
        }
    }
    with_free(cols, binds, checks)
}

/// `cols` with the free columns a step binds or checks filled in.
fn with_free(
    mut cols: Vec<Option<KeyPart>>,
    binds: &[(u8, u16)],
    checks: &[(u8, KeyPart)],
) -> Vec<Option<KeyPart>> {
    for &(c, v) in binds {
        if let Some(slot) = cols.get_mut(usize::from(c)) {
            *slot = Some(KeyPart::Var(v));
        }
    }
    for (c, part) in checks {
        if let Some(slot) = cols.get_mut(usize::from(*c)) {
            *slot = Some(part.clone());
        }
    }
    cols
}

fn term_is(t: &CTerm, c: &Option<KeyPart>) -> bool {
    match (t, c) {
        (CTerm::Var(v), Some(KeyPart::Var(w))) => v == w,
        (CTerm::Const(a), Some(KeyPart::Const(b))) => a == b,
        (CTerm::Wild, None) => true,
        _ => false,
    }
}

fn columns_are(terms: &[CTerm], cols: &[Option<KeyPart>]) -> bool {
    terms.len() == cols.len() && terms.iter().zip(cols).all(|(t, c)| term_is(t, c))
}

/// A reaches literal's two ends.
fn ends_are(terms: &[CTerm], ends: &[Option<KeyPart>]) -> bool {
    terms.len() >= 2 && terms.iter().zip(ends).all(|(t, c)| term_is(t, c))
}

/// A `reaches/4` call's hop bound, as the planner reads it.
fn depth_of(terms: &[CTerm]) -> Option<u32> {
    match terms.get(3) {
        Some(CTerm::Const(ConstVal::Int(n))) => {
            Some(u32::try_from(*n).unwrap_or(if *n < 0 { 0 } else { u32::MAX }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use glia_activation::algo::GraphSource;
    use glia_core::{Confidence, NodeKindId};

    use super::*;
    use crate::eval::{Options, run};
    use crate::source::MemSource;
    use crate::testkit::{GraphSpec, category_id, chain, lcg_graph, node_id, resalted, tables};

    const FUNCTION: NodeKindId = NodeKindId(1);
    const ROUTE: NodeKindId = NodeKindId(3);

    const TC: &str = r#"tc(x, y) :- edge(x, y, "CALLS").
tc(x, z) :- tc(x, y), edge(y, z, "CALLS").
"#;

    /// Spec test 10's seeded chain walk.
    const WALK: &str = r#"r(y) :- node(y, _), index(y, 0).
r(z) :- r(y), edge(y, z, "CALLS").
"#;

    fn eval<'s>(text: &str, g: &'s MemSource, opts: &Options) -> Evaluation<'s, MemSource> {
        run("rules", text, g, opts).unwrap_or_else(|e| panic!("evaluation failed: {e}"))
    }

    /// The row of `pred` whose rendered values, space-joined, are `want`.
    fn row_of(ev: &Evaluation<'_, MemSource>, pred: &str, want: &str) -> Row {
        let rows = ev.rows(pred).unwrap_or_else(|e| panic!("rows({pred}): {e}"));
        let text = |r: &Row| r.values.iter().map(|v| ev.render(v)).collect::<Vec<_>>().join(" ");
        rows.iter()
            .find(|r| text(r) == want)
            .cloned()
            .unwrap_or_else(|| panic!("no {pred}({want})"))
    }

    fn explain(ev: &mut Evaluation<'_, MemSource>, row: &Row, limits: &ExplainLimits) -> Witness {
        ev.explain(row, limits).unwrap_or_else(|e| panic!("explain failed: {e}"))
    }

    /// `n` numbered testkit nodes (`n<i:05>`, `index` = i, salt 0) and no edges.
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

    /// A source of named nodes (`id(i)` is `node_id(0, i)`) and CALLS edges by name.
    fn named(names: &[(&str, NodeKindId)], edges: &[(&str, &str, Tier)]) -> MemSource {
        let mut g = MemSource::new(tables());
        for (i, (name, kind)) in names.iter().enumerate() {
            g.node(node_id(0, i as u32), *kind, *name);
        }
        let at = |n: &str| node_id(0, names.iter().position(|x| x.0 == n).unwrap_or(0) as u32);
        for (from, to, tier) in edges {
            g.edge(at(from), at(to), category_id("CALLS"), Confidence::Strong, *tier);
        }
        g
    }

    fn hop(from: u32, to: u32, ordinal: usize, tier: Tier) -> EdgeHop {
        EdgeHop {
            from: node_id(0, from),
            to: node_id(0, to),
            category: category_id("CALLS"),
            ordinal,
            tier,
        }
    }

    /// Each leaf edge as (render(from), render(to), category, tier): what survives a
    /// checkout.
    fn rendered_hops<S: FactSource + ?Sized>(src: &S, w: &Witness) -> Vec<String> {
        let reg = src.tables().registries;
        w.leaf_edges()
            .iter()
            .map(|h| {
                let cat = reg.category_name(h.category).unwrap_or("?");
                format!(
                    "{}->{} {cat} {}",
                    src.render_node(h.from),
                    src.render_node(h.to),
                    h.tier.name()
                )
            })
            .collect()
    }

    /// The witness as indented lines of rendered values, leaves and flags.
    fn outline(ev: &Evaluation<'_, MemSource>, w: &Witness) -> Vec<String> {
        let reg = ev.src.tables().registries;
        let r = |id: &NodeId| ev.src.render_node(*id);
        let hop = |h: &EdgeHop| {
            let cat = reg.category_name(h.category).unwrap_or("?");
            format!("{}->{} {cat} {}", r(&h.from), r(&h.to), h.tier.name())
        };
        let mut out = Vec::new();
        let mut stack = vec![(w, 0usize)];
        while let Some((n, depth)) = stack.pop() {
            let vals: Vec<String> = n.values.iter().map(|v| ev.render(v)).collect();
            let neg = if n.negated { "!" } else { "" };
            let mut line = format!(
                "{}{neg}{}({}) {}",
                "  ".repeat(depth),
                n.predicate,
                vals.join(", "),
                n.tier.name()
            );
            match &n.leaf {
                Some(Leaf::Edge(h)) => line.push_str(&format!(" edge {}", hop(h))),
                Some(Leaf::Path(hops)) => {
                    let hs: Vec<String> = hops.iter().map(hop).collect();
                    line.push_str(&format!(" path [{}]", hs.join(", ")));
                }
                Some(Leaf::Node { id, kind }) => line.push_str(&format!(" node {} {kind}", r(id))),
                Some(Leaf::Attr { name, value, .. }) => {
                    line.push_str(&format!(" attr {name}={}", ev.render(value)));
                }
                Some(other) => line.push_str(&format!(" {other:?}")),
                None => {}
            }
            if let Some(rule) = &n.rule {
                line.push_str(&format!(" h={} @{}:{}", n.height, rule.source, rule.line));
            }
            if n.truncated {
                line.push_str(" truncated");
            }
            out.push(line);
            stack.extend(n.support.iter().rev().map(|c| (c, depth + 1)));
        }
        out
    }

    /// Every predicate the witness names, depth-first.
    fn predicates(w: &Witness) -> Vec<String> {
        let (mut out, mut stack) = (Vec::new(), vec![w]);
        while let Some(n) = stack.pop() {
            out.push(n.predicate.clone());
            stack.extend(n.support.iter().rev());
        }
        out
    }

    #[test]
    fn tc_witness_is_a_shortest_path() {
        let g = chain(4, "CALLS", &[Tier::Fact]);
        let mut ev = eval(TC, &g, &Options::default());
        let row = row_of(&ev, "tc", "n00000 n00003");
        let w = explain(&mut ev, &row, &ExplainLimits::default());
        let want =
            vec![hop(0, 1, 0, Tier::Fact), hop(1, 2, 1, Tier::Fact), hop(2, 3, 2, Tier::Fact)];
        assert_eq!(w.leaf_edges(), want);
        assert_eq!((w.predicate.as_str(), w.height, w.tier), ("tc/2", 3, Tier::Fact));
        assert_eq!(w.rule, Some(RuleRef { index: 1, source: "rules".into(), line: 2 }));
        assert_eq!(
            outline(&ev, &w),
            [
                "tc/2(n00000, n00003) fact h=3 @rules:2",
                "  tc/2(n00000, n00002) fact h=2 @rules:2",
                "    tc/2(n00000, n00001) fact h=1 @rules:1",
                "      edge/3(n00000, n00001, CALLS) fact edge n00000->n00001 CALLS fact",
                "    edge/3(n00001, n00002, CALLS) fact edge n00001->n00002 CALLS fact",
                "  edge/3(n00002, n00003, CALLS) fact edge n00002->n00003 CALLS fact",
            ]
        );
        assert!(!w.truncated);
    }

    #[test]
    fn fact_long_path_beats_heuristic_short() {
        // a = 0, b = 1, c = 2, d = 3: a -> d HEURISTIC first in edge order.
        let mut g = nodes(4);
        edge(&mut g, 0, 3, "CALLS", Tier::Heuristic);
        edge(&mut g, 0, 1, "CALLS", Tier::Fact);
        edge(&mut g, 1, 2, "CALLS", Tier::Fact);
        edge(&mut g, 2, 3, "CALLS", Tier::Fact);
        let mut ev = eval(TC, &g, &Options::default());
        let row = row_of(&ev, "tc", "n00000 n00003");
        assert_eq!(row.tier, Tier::Fact);
        let w = explain(&mut ev, &row, &ExplainLimits::default());
        assert_eq!(
            w.leaf_edges(),
            [hop(0, 1, 1, Tier::Fact), hop(1, 2, 2, Tier::Fact), hop(2, 3, 3, Tier::Fact)]
        );
        assert_eq!(w.tier, Tier::Fact);
    }

    #[test]
    fn checkout_stable_choice() {
        // Two FACT derivations of tc(a, d) at height 2, through b and through c; c
        // renders first, b comes first in edge and handle order.
        let names = [("a", FUNCTION), ("x1", FUNCTION), ("x0", FUNCTION), ("d", FUNCTION)];
        let edges = [
            ("a", "x1", Tier::Fact),
            ("x1", "d", Tier::Fact),
            ("a", "x0", Tier::Fact),
            ("x0", "d", Tier::Fact),
        ];
        let g = named(&names, &edges);
        let g2 = resalted(&g, 99);
        let mut ev = eval(TC, &g, &Options::default());
        let row = row_of(&ev, "tc", "a d");
        let w = explain(&mut ev, &row, &ExplainLimits::default());
        assert_eq!(rendered_hops(&g, &w), ["a->x0 CALLS fact", "x0->d CALLS fact"]);
        let mut ev2 = eval(TC, &g2, &Options::default());
        let row2 = row_of(&ev2, "tc", "a d");
        let w2 = explain(&mut ev2, &row2, &ExplainLimits::default());
        assert_eq!(outline(&ev2, &w2), outline(&ev, &w));
        assert_eq!(rendered_hops(&g2, &w2), rendered_hops(&g, &w));
        // The ordinals differ: the edge order is reversed.
        assert_ne!(w2.leaf_edges()[0].ordinal, w.leaf_edges()[0].ordinal);
    }

    #[test]
    fn negation_node() {
        let names = [("r1", ROUTE), ("r2", ROUTE), ("f1", FUNCTION)];
        let mut g = named(&names, &[]);
        g.edge(
            node_id(0, 0),
            node_id(0, 2),
            category_id("HANDLED_BY"),
            Confidence::Strong,
            Tier::Fact,
        );
        let text = r#"handled(r) :- edge(r, _, "HANDLED_BY").
unhandled(r) :- node(r, "ROUTE"), !handled(r).
"#;
        let mut ev = eval(text, &g, &Options::default());
        let row = row_of(&ev, "unhandled", "r2");
        let w = explain(&mut ev, &row, &ExplainLimits::default());
        assert_eq!(w.support.len(), 2, "{w:?}");
        let node = &w.support[0];
        assert_eq!(node.leaf, Some(Leaf::Node { id: node_id(0, 1), kind: "ROUTE".into() }));
        let neg = &w.support[1];
        assert_eq!(
            (
                neg.predicate.as_str(),
                neg.negated,
                neg.support.len(),
                neg.rule.is_none(),
                neg.leaf.is_none()
            ),
            ("handled/1", true, 0, true, true)
        );
        assert_eq!(neg.values, [Value::Node(node_id(0, 1))]);
        assert!(w.leaf_edges().is_empty());
    }

    #[test]
    fn aggregate_support() {
        // n00003's callers, in edge order n00004, n00000, n00001; n00002 calls n00004.
        let mut g = nodes(5);
        for (f, t) in [(4, 3), (0, 3), (1, 3), (2, 4)] {
            edge(&mut g, f, t, "CALLS", Tier::Fact);
        }
        let mut ev = eval(r#"fan(y, count(x)) :- edge(x, y, "CALLS")."#, &g, &Options::default());
        let row = row_of(&ev, "fan", "n00003 3");
        let w = explain(&mut ev, &row, &ExplainLimits::default());
        assert_eq!(w.rule, Some(RuleRef { index: 0, source: "rules".into(), line: 1 }));
        assert_eq!(
            outline(&ev, &w),
            [
                "fan/2(n00003, 3) fact h=1 @rules:1",
                "  fan/2(n00003, n00000) fact h=0 @rules:1",
                "    edge/3(n00000, n00003, CALLS) fact edge n00000->n00003 CALLS fact",
                "  fan/2(n00003, n00001) fact h=0 @rules:1",
                "    edge/3(n00001, n00003, CALLS) fact edge n00001->n00003 CALLS fact",
                "  fan/2(n00003, n00004) fact h=0 @rules:1",
                "    edge/3(n00004, n00003, CALLS) fact edge n00004->n00003 CALLS fact",
            ]
        );
    }

    #[test]
    fn reaches_path_leaf() {
        // Two ROUTE seeds: a1 (renders first) reaches t in 3 hops, a2 in 1.
        let names =
            [("a1", ROUTE), ("a2", ROUTE), ("x", FUNCTION), ("y", FUNCTION), ("t", FUNCTION)];
        let edges = [
            ("a1", "x", Tier::Fact),
            ("x", "y", Tier::Fact),
            ("y", "t", Tier::Fact),
            ("a2", "t", Tier::Fact),
        ];
        let g = named(&names, &edges);
        let text = r#"far(b) :- node(a, "ROUTE"), reaches(a, b, "CALLS")."#;
        let mut ev = eval(text, &g, &Options::default());
        let hoisted = ev.plans.iter().flat_map(|p| &p.rules).flat_map(|r| &r.variants);
        assert!(
            hoisted.into_iter().any(|v| matches!(v.steps.first(), Some(Step::ReachMulti { .. }))),
            "far's evaluation plan hoists the reach"
        );
        let row = row_of(&ev, "far", "t");
        let w = explain(&mut ev, &row, &ExplainLimits::default());
        assert_eq!(
            outline(&ev, &w),
            [
                "far/1(t) fact h=1 @rules:1",
                "  node/2(a2, ROUTE) fact node a2 ROUTE",
                "  reaches/3(a2, t, CALLS) fact path [a2->t CALLS fact]",
            ]
        );
        assert_eq!(w.leaf_edges(), [hop(1, 4, 3, Tier::Fact)]);
        // A node only a1 reaches is explained from a1.
        let row = row_of(&ev, "far", "y");
        let w = explain(&mut ev, &row, &ExplainLimits::default());
        assert_eq!(rendered_hops(&g, &w), ["a1->x CALLS fact", "x->y CALLS fact"]);
    }

    #[test]
    fn nested_hoists_and_unbound_reach_end() {
        // ROUTE a1 -CALLS-> m1 -CALLS-> m2 <-CALLS- ROUTE a2; m2 -USES-> u1 -USES-> c;
        // `index` numbers the nodes in that order.
        let names = [
            ("a1", ROUTE),
            ("a2", ROUTE),
            ("m1", FUNCTION),
            ("m2", FUNCTION),
            ("u1", FUNCTION),
            ("c", FUNCTION),
        ];
        let mut g = named(
            &names,
            &[("a1", "m1", Tier::Fact), ("m1", "m2", Tier::Fact), ("a2", "m2", Tier::Fact)],
        );
        for (f, t) in [(3, 4), (4, 5)] {
            g.edge(
                node_id(0, f),
                node_id(0, t),
                category_id("USES"),
                Confidence::Strong,
                Tier::Fact,
            );
        }
        g.attr(crate::testkit::INDEX);
        for i in 0..6 {
            g.set_attr(node_id(0, i), 0, Value::Int(i64::from(i)));
        }
        // deep: the second reach folds the first into its seed. step: the reached end
        // `b` is no head variable, so the bindings are filtered on `i` alone.
        let text = r#"deep(c) :- node(a, "ROUTE"), reaches(a, b, "CALLS"), reaches(b, c, "USES").
step(i) :- node(a, "ROUTE"), reaches(a, b, "CALLS"), index(b, i).
"#;
        let mut ev = eval(text, &g, &Options::default());
        let base = |ev: &Evaluation<'_, MemSource>, rule: usize| {
            let r = ev.plans.iter().flat_map(|p| &p.rules).find(|r| r.rule == rule);
            r.and_then(|r| r.variants.first()).and_then(|v| v.steps.first()).cloned()
        };
        let nested = matches!(base(&ev, 0), Some(Step::ReachMulti { seed, .. })
            if matches!(seed.first(), Some(Step::ReachMulti { .. })));
        assert!(nested, "deep's plan nests one hoist in another: {:?}", base(&ev, 0));
        let hoisted = matches!(base(&ev, 1), Some(Step::ReachMulti { bind: 2, .. }));
        assert!(hoisted, "step's reach hoists, binding b: {:?}", base(&ev, 1));
        let row = row_of(&ev, "deep", "c");
        let w = explain(&mut ev, &row, &ExplainLimits::default());
        assert_eq!(
            outline(&ev, &w),
            [
                "deep/1(c) fact h=1 @rules:1",
                "  node/2(a2, ROUTE) fact node a2 ROUTE",
                "  reaches/3(a2, m2, CALLS) fact path [a2->m2 CALLS fact]",
                "  reaches/3(m2, c, USES) fact path [m2->u1 USES fact, u1->c USES fact]",
            ]
        );
        let row = row_of(&ev, "step", "3");
        let w = explain(&mut ev, &row, &ExplainLimits::default());
        assert_eq!(
            outline(&ev, &w),
            [
                "step/1(3) fact h=1 @rules:2",
                "  node/2(a2, ROUTE) fact node a2 ROUTE",
                "  reaches/3(a2, m2, CALLS) fact path [a2->m2 CALLS fact]",
                "  index/2(m2, 3) fact attr index=3",
            ]
        );
    }

    #[test]
    fn leaf_kinds() {
        let mut g = nodes(4);
        edge(&mut g, 0, 1, "CALLS", Tier::Derived);
        let text = r#"p(x, k) :- node(x, k), kind(k), index(x, i), i < 1, starts_with(k, "FUN"), category("CALLS"), edge_conf(x, _, "CALLS", c).
"#;
        let mut ev = eval(text, &g, &Options::default());
        let row = row_of(&ev, "p", "n00000 FUNCTION");
        let w = explain(&mut ev, &row, &ExplainLimits::default());
        let leaves: Vec<(&str, Option<&Leaf>)> =
            w.support.iter().map(|c| (c.predicate.as_str(), c.leaf.as_ref())).collect();
        let n0 = node_id(0, 0);
        assert_eq!(
            leaves,
            [
                ("node/2", Some(&Leaf::Node { id: n0, kind: "FUNCTION".into() })),
                ("kind/1", Some(&Leaf::Registry)),
                (
                    "index/2",
                    Some(&Leaf::Attr { id: n0, name: "index".into(), value: Value::Int(0) })
                ),
                (
                    "starts_with/2",
                    Some(&Leaf::Builtin {
                        name: "starts_with".into(),
                        args: vec![Value::Str("FUNCTION".into()), Value::Str("FUN".into())]
                    })
                ),
                ("category/1", Some(&Leaf::Registry)),
                ("edge_conf/4", Some(&Leaf::Edge(hop(0, 1, 0, Tier::Derived)))),
            ]
        );
        assert_eq!(w.tier, Tier::Derived);
        assert_eq!(w.support[5].values[3], Value::Str("strong".into()));
    }

    #[test]
    fn scc_cycle_leaf() {
        // 0 -> 1 -> 2 -> 0 and 2 -> 3.
        let mut g = nodes(4);
        for (f, t) in [(0, 1), (1, 2), (2, 0), (2, 3)] {
            edge(&mut g, f, t, "CALLS", Tier::Fact);
        }
        let mut ev = eval(r#"cyc(x, r) :- scc(x, r, "CALLS")."#, &g, &Options::default());
        let row = row_of(&ev, "cyc", "n00001 n00000");
        let w = explain(&mut ev, &row, &ExplainLimits::default());
        let leaf = &w.support[0];
        assert_eq!(
            leaf.values,
            [Value::Node(node_id(0, 1)), Value::Node(node_id(0, 0)), Value::Str("CALLS".into())]
        );
        let Some(Leaf::Path(hops)) = &leaf.leaf else { panic!("no path leaf: {leaf:?}") };
        assert_eq!(
            hops,
            &[hop(0, 1, 0, Tier::Fact), hop(1, 2, 1, Tier::Fact), hop(2, 0, 2, Tier::Fact)]
        );
        assert_eq!(
            (hops[0].from, hops[2].to),
            (node_id(0, 0), node_id(0, 0)),
            "starts and ends at rep"
        );
    }

    #[test]
    fn max_leaves() {
        let g = chain(151, "CALLS", &[Tier::Fact]);
        let mut ev = eval(TC, &g, &Options::default());
        let row = row_of(&ev, "tc", "n00000 n00150");
        let limits = ExplainLimits { max_leaves: 100, ..ExplainLimits::default() };
        let w = explain(&mut ev, &row, &limits);
        let hops = w.leaf_edges();
        assert_eq!(hops.len(), 100);
        assert_eq!(
            hops,
            (0..100).map(|i| hop(i, i + 1, i as usize, Tier::Fact)).collect::<Vec<_>>()
        );
        assert!(w.truncated, "the root's last edge was dropped");
        // Under the default cap of 100 the 150 hops are cut the same way; with room
        // for all of them nothing is.
        let all = explain(&mut ev, &row, &ExplainLimits { max_leaves: 150, ..limits });
        assert_eq!(all.leaf_edges().len(), 150);
        assert!(!all.truncated);
    }

    #[test]
    fn magic_elided() {
        let g = chain(6, "CALLS", &[Tier::Fact]);
        let text = format!("seed(x) :- index(x, 0).\n{TC}out(b) :- seed(a), tc(a, b).\n");
        let mut on = eval(&text, &g, &Options::default());
        let mut off = eval(&text, &g, &Options { magic: false, ..Options::default() });
        assert_eq!(on.stats().magic_rewrites, 1);
        let (r_on, r_off) = (row_of(&on, "out", "n00005"), row_of(&off, "out", "n00005"));
        let w_on = explain(&mut on, &r_on, &ExplainLimits::default());
        let w_off = explain(&mut off, &r_off, &ExplainLimits::default());
        let names = predicates(&w_on);
        assert!(names.iter().all(|p| !p.starts_with("__")), "{names:?}");
        assert!(names.iter().any(|p| p == "seed/1"), "{names:?}");
        assert_eq!(w_on.leaf_edges().len(), 5);
        assert_eq!(w_on.leaf_edges(), w_off.leaf_edges());
        assert_eq!(w_on.rule.as_ref().map(|r| r.line), Some(4));
    }

    #[test]
    fn deep_chain_no_stack_overflow() {
        let g = chain(20_001, "CALLS", &[Tier::Fact]);
        let mut ev = eval(WALK, &g, &Options::default());
        let rows = ev.rows("r").unwrap_or_default();
        assert_eq!(rows.len(), 20_001);
        let row = row_of(&ev, "r", "n20000");
        let limits = ExplainLimits { max_leaves: 30_000, ..ExplainLimits::default() };
        let w = explain(&mut ev, &row, &limits);
        assert_eq!(w.leaf_edges().len(), 20_000);
        assert!(!w.truncated);
        assert_eq!(w.height, 20_001);
        // Clone, equality and drop walk the 20,001-deep tree on explicit stacks.
        let copy = w.clone();
        assert!(copy == w);
    }

    /// The permutations of a 3-column relation that have an index.
    fn indexed(rel: &Relation) -> Vec<Vec<u8>> {
        let perms = [[0, 1, 2], [0, 2, 1], [1, 0, 2], [1, 2, 0], [2, 0, 1], [2, 1, 0]];
        perms.iter().filter(|p| rel.index_of(&p[..]).is_some()).map(|p| p.to_vec()).collect()
    }

    #[test]
    fn explain_builds_missing_index() {
        let g = chain(201, "CALLS", &[Tier::Fact]);
        let mut ev = eval(WALK, &g, &Options::default());
        let edge = ev.analysis.preds.iter().position(|p| p.name == "edge").unwrap_or(usize::MAX);
        let before = indexed(&ev.relations[edge]);
        assert!(!before.contains(&vec![1, 2, 0]), "{before:?}");
        let row = row_of(&ev, "r", "n00200");
        let limits = ExplainLimits { max_leaves: 1_000, ..ExplainLimits::default() };
        let first = explain(&mut ev, &row, &limits);
        let after = indexed(&ev.relations[edge]);
        assert_eq!(after.len(), before.len() + 1);
        assert!(after.contains(&vec![1, 2, 0]));
        let at = ev.relations[edge].index_of(&[1, 2, 0]);
        let second = explain(&mut ev, &row, &limits);
        assert_eq!(indexed(&ev.relations[edge]), after, "the second explain builds nothing");
        assert_eq!(ev.relations[edge].index_of(&[1, 2, 0]), at);
        assert!(first == second);
        assert_eq!(first.leaf_edges().len(), 200);
    }

    #[test]
    fn candidates_budget() {
        // p(n00000) has 50 equal-height derivations.
        let mut g = nodes(51);
        for t in 1..=50 {
            edge(&mut g, 0, t, "CALLS", Tier::Fact);
        }
        let mut ev = eval(r#"p(x) :- edge(x, y, "CALLS")."#, &g, &Options::default());
        let row = row_of(&ev, "p", "n00000");
        let limits = ExplainLimits { max_candidates: 10, ..ExplainLimits::default() };
        let err = ev.explain(&row, &limits).map(|w| w.height);
        assert_eq!(
            err,
            Err(RulesError::Budget {
                predicate: "p/1".into(),
                source: "rules".into(),
                line: 1,
                rows: 11,
                cap: 10
            })
        );
        // The evaluation's own budget is the default cap, and under it the smallest
        // rendered target wins.
        assert_eq!(ev.explain_limits().max_candidates, DEFAULT_MAX_ROWS);
        let own = ev.explain_limits();
        let w = explain(&mut ev, &row, &own);
        assert_eq!(w.leaf_edges(), [hop(0, 1, 0, Tier::Fact)]);
    }

    #[test]
    fn foreign_row_is_unknown() {
        let (g1, g2) = (chain(3, "CALLS", &[Tier::Fact]), chain(5, "CALLS", &[Tier::Derived]));
        let ev1 = eval(TC, &g1, &Options::default());
        let mut ev2 = eval(TC, &g2, &Options::default());
        let row = row_of(&ev1, "tc", "n00000 n00002");
        let err = ev2.explain(&row, &ExplainLimits::default()).map(|w| w.height);
        assert_eq!(
            err,
            Err(RulesError::Unknown { predicate: "tc/2".into(), known: vec!["tc/2".into()] })
        );
    }

    #[test]
    fn explain_all_and_marker() {
        let g = chain(4, "CALLS", &[Tier::Fact, Tier::Derived]);
        let mut ev = eval(TC, &g, &Options::default());
        let rows = ev.rows("tc").unwrap_or_default();
        let all = ev.explain_all(&rows, &ExplainLimits::default()).unwrap_or_default();
        assert_eq!(all.len(), rows.len());
        for (r, w) in rows.iter().zip(&all) {
            assert_eq!((w.tier, w.height), (r.tier, r.at.meta.height));
            assert!(w.leaf_edges().iter().all(|h| h.tier <= r.tier));
            assert_eq!(&explain(&mut ev, r, &ExplainLimits::default()), w);
        }
        assert_eq!(
            explain_line("tc/2", Tier::Derived, 3, 3, 4, false),
            "[rules] explain pred=tc/2 tier=derived height=3 leaves=3 candidates=4 truncated=false"
        );
    }

    #[test]
    fn witness_debug_is_flat() {
        let g = chain(3, "CALLS", &[Tier::Fact]);
        let mut ev = eval(TC, &g, &Options::default());
        let row = row_of(&ev, "tc", "n00000 n00001");
        let w = explain(&mut ev, &row, &ExplainLimits::default());
        let text = format!("{w:?}");
        assert!(text.starts_with("Witness { predicate: \"tc/2\", "), "{text}");
        assert!(text.contains("support: [Witness { predicate: \"edge/3\""), "{text}");
        assert!(text.ends_with("support: [] }] }"), "{text}");
        assert_eq!(g.edges().count(), 2);
    }
}
