//! The GQL read subset lowered to the rule IR (DB.7b). [`compile`] parses a query
//! (DB.7a) and lowers it to a [`Program`] whose output predicate `result` holds the
//! query's rows, so GQL and the Datalog surface share one evaluator, one tier lattice
//! and one row budget (R1 §0.5: GQL is R2 on the same IR). Domain-free: labels and
//! relationship types are names analysis resolves against the FactSource registries
//! (an unknown one is its error, located at the label), properties are attributes.
//!
//! **The mapping** (R1 §5). Every pattern variable is an IR variable of the same name;
//! an anonymous node, an attribute value and an unrolled intermediate get a fresh
//! `_g<n>` (skipping any name the query uses).
//! - `(v:L)` is `node(v, "L")`; `(v:L1|L2)` one alternative per label; `{k: lit}` the
//!   attribute atom `k(v, lit)`. A node no positive atom binds gets `node(v, _)`.
//! - `-[:T]->` is `edge(a, b, "T")`, `<-` swaps the ends, `-[]-` is two alternatives
//!   (a -> b, b -> a); no type is `edge(a, b, _)`; several types are one helper
//!   `gql_hop<n>(x, y) :- edge(x, y, "T1").`, one rule per type. A relationship
//!   variable `r` binds the category, so `type(r)` reads it.
//! - `*` / `+` / `{1,}` is `reaches(a, b, "T1|T2")` (`"*"` untyped): plain
//!   reachability, never unrolled, so DA's seeded closure and row budget apply.
//!   `{0,}` adds the alternative `a = b`; `{m,}` is m - 1 hops then reaches; `{m,n}`
//!   (n <= 8) is one alternative per length, unrolled with fresh intermediates: WALK
//!   length, so `*2..2` from f1 over f1 -> f2 -> f3 and f1 -> f4 is {f3} although f4
//!   is also reached in 3 hops. A variable-length relationship needs a direction; a
//!   bounded undirected one unrolls through a two-way hop helper.
//! - WHERE goes to negation normal form (De Morgan; NOT flips a comparison), then DNF:
//!   each conjunct is one rule body with the MATCH atoms. NOT over a pattern / EXISTS,
//!   a string test or a function call is a helper `gql_not<n>` whose head is the
//!   variables it shares with the enclosing query, negated. `IN [..]` is a fact table
//!   `gql_in<n>(lit).`. A positive pattern predicate is inlined (its own variables are
//!   local). A function call is the builtin of that name (`in_scope`).
//! - RETURN is the head `result(..)`; `a.k` binds `k(a, X)`, `type(r)` the category.
//!   Aggregates are DA's grouped head over the non-aggregate items; `count(*)` counts
//!   the distinct bindings of every MATCH variable. An aggregate over several
//!   alternatives reads their union `gql_match<n>`, since an aggregate predicate has
//!   exactly one rule.
//!
//! **Semantics (Q4).** Rows are DISTINCT (`RETURN DISTINCT` changes nothing; DA's
//! `count` counts distinct values). Two relationship patterns may bind one edge (edge
//! homomorphism, unlike openCypher's isomorphism). No nulls: a node without an
//! attribute the query reads drops the row, under NOT too (`NOT a.k = 'x'` keeps only
//! nodes whose `k` is not `x`).
//!
//! **Path column.** `p = [ANY SHORTEST] (a)..(b)` with exactly one variable-length
//! relationship returns `p` as [`Compiled::path_column`]. The program's head holds the
//! path's start node in that column plus ONE trailing column after `columns`, its end
//! node, so two paths with different endpoints stay two rows; the engine (DB.7c)
//! replaces the column with the row's witness and shows `columns.len()` values. Glia
//! returns endpoint pairs, so ANY SHORTEST and a bare path variable lower alike.
//!
//! **Limits**, each a located diagnostic: more than 64 alternatives, a bounded
//! repetition above 8 hops, an unrolled lower bound above 8 hops, an undirected
//! variable-length relationship, and the reserved names (`result` and the `gql_`
//! prefix, which no prelude predicate uses) as a property key or function name.
//!
//! Helpers are numbered by visit order (MATCH, WHERE, RETURN) and the program lists
//! the `result` rules, then the helpers in creation order, so one text lowers to one
//! program. Every map is a `Vec` (no hash map). fired_on: `[gql] lower rules=<r>
//! helpers=<h> alternatives=<a> output=result` on stderr once per [`compile`]; a
//! failed compile prints zeros and ` diagnostics=<d>`.

use crate::diag::{Diagnostic, Span};
use crate::ir::{Aggregate, Atom, CmpOp, Comparison, Head, HeadArg, Literal, Program, Rule, Term};
use crate::lexer::MAX_DIAGNOSTICS;

use super::ast::{
    Dir, Expr, Ident, Lit, NodePat, Operand, PathPattern, Quant, Query, RelPat, RetExpr, StrOp,
};
use super::{SOURCE_NAME, parse};

/// The output predicate.
const OUTPUT: &str = "result";
/// The prefix of every helper predicate.
const PREFIX: &str = "gql_";
/// The most alternatives (rule bodies) one query or helper may expand to.
const MAX_ALTERNATIVES: u64 = 64;
/// The most hops a repetition unrolls.
const MAX_UNROLL: u32 = 8;
/// The core string builtins: they need their arguments bound, so they bind nothing.
const STRING_TESTS: [&str; 3] = ["starts_with", "ends_with", "contains"];
const BOOL_LITERAL: &str = "boolean literals are not rule values (the rule IR holds nodes, \
                            strings and integers): compare the attribute's own value";

/// A compiled GQL read query: the program and what the engine applies after it.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    /// Source 0 is [`SOURCE_NAME`], the source every span points into.
    pub program: Program,
    /// `"result"`.
    pub output: String,
    /// One per RETURN item: its alias, else its source text (`a.qname`, `count(b)`).
    pub columns: Vec<String>,
    pub order_by: Vec<OrderKey>,
    /// LIMIT, for post-processing after ORDER BY (the engine sorts rendered values).
    pub limit: Option<u64>,
    /// The column a path variable fills; its head holds one trailing column more
    /// (module doc).
    pub path_column: Option<usize>,
}

/// One ORDER BY key, resolved to a column of [`Compiled::columns`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderKey {
    pub column: usize,
    pub descending: bool,
}

/// Parse `text` and lower it (module doc). Parse errors come back unchanged.
pub fn compile(text: &str) -> Result<Compiled, Vec<Diagnostic>> {
    let failed = |n: usize| {
        eprintln!("[gql] lower rules=0 helpers=0 alternatives=0 output={OUTPUT} diagnostics={n}");
    };
    let query = match parse(text) {
        Ok(q) => q,
        Err(diags) => {
            failed(diags.len());
            return Err(diags);
        }
    };
    let mut lw = Lower::new(text);
    let lowered = lw.query(&query);
    match lowered {
        Some((c, alternatives)) if lw.diags.is_empty() => {
            eprintln!(
                "[gql] lower rules={} helpers={} alternatives={alternatives} output={OUTPUT}",
                c.program.rules.len(),
                lw.helper_preds.len()
            );
            Ok(c)
        }
        _ => {
            if lw.diags.is_empty() {
                lw.err(query.span, "the query could not be lowered");
            }
            lw.diags.sort_by_key(|d| (d.span.start, d.span.end));
            failed(lw.diags.len());
            Err(lw.diags)
        }
    }
}

// ============================================================================
// State
// ============================================================================

/// A rule body: a conjunction of literals.
type Conj = Vec<Literal>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VarKind {
    Node,
    /// An unquantified relationship: its variable is the edge category.
    Rel,
    /// A variable-length relationship: no single category, so never bound.
    QuantRel,
    Path,
}

impl VarKind {
    fn describe(self) -> &'static str {
        match self {
            VarKind::Node => "a node variable",
            VarKind::Rel => "a relationship variable",
            VarKind::QuantRel => "a variable-length relationship variable",
            VarKind::Path => "a path variable",
        }
    }
}

/// How range restriction binds a variable no positive atom binds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bind {
    /// `node(v, _)`.
    Node,
    /// `category(v)`.
    Cat,
}

/// A lowered WHERE / RETURN operand.
struct Val {
    term: Term,
    /// The attribute atoms its value needs, positive (no nulls).
    pre: Conj,
    /// What a condition helper's head carries, with how its body binds each (`None`:
    /// an attribute value, bound by its `pre` atom).
    owns: Vec<(String, Option<Bind>)>,
}

/// Where a pattern's variables live.
enum Scope {
    /// MATCH: by the query's names.
    Match,
    /// A WHERE pattern: a MATCH node variable is shared, any other name is local and
    /// renamed fresh.
    Local { renames: Vec<(String, String)>, shared: Vec<String> },
}

/// WHERE in negation normal form: a leaf is an atomic condition and whether it is
/// negated.
enum Nnf<'q> {
    Or(Vec<Nnf<'q>>),
    And(Vec<Nnf<'q>>),
    Leaf(&'q Expr, bool),
}

/// A path variable and the IR variables of its pattern's two ends.
struct PathVar {
    name: String,
    start: String,
    end: String,
}

#[derive(Clone, Copy)]
enum Helper {
    Hop,
    Not,
    In,
    Match,
}

struct Lower<'q> {
    text: &'q str,
    /// MATCH variables by name, in first-appearance order.
    vars: Vec<(String, VarKind)>,
    /// MATCH node variables (named and anonymous), in visit order.
    match_nodes: Vec<String>,
    /// Every node variable minted, MATCH or WHERE.
    node_vars: Vec<String>,
    paths: Vec<PathVar>,
    /// `(node variable, key, value variable)`: one value variable per attribute read.
    attrs: Vec<(String, String, String)>,
    /// Function-call names: domain builtins whose binding behaviour is unknown here.
    calls: Vec<String>,
    fresh: u32,
    helpers: Vec<Rule>,
    helper_preds: Vec<String>,
    /// Hop helpers by (types, category column, two-way).
    hops: Vec<(String, String)>,
    /// Next number per [`Helper`].
    counters: [u32; 4],
    diags: Vec<Diagnostic>,
}

// ============================================================================
// Small constructors
// ============================================================================

fn atom(pred: &str, args: Vec<Term>, span: Span) -> Atom {
    Atom { pred: pred.to_string(), args, span }
}

fn str_at(value: &str, span: Span) -> Term {
    Term::Str { value: value.to_string(), span }
}

fn eq(left: Term, right: Term, span: Span) -> Literal {
    Literal::Cmp(Comparison { op: CmpOp::Eq, left, right, span })
}

fn node_any(v: &str, span: Span) -> Literal {
    Literal::Pos(atom("node", vec![Term::var(v), Term::wild()], span))
}

fn rule(pred: &str, args: Vec<HeadArg>, body: Conj, span: Span) -> Rule {
    Rule { head: Head { pred: pred.to_string(), args, span }, body, span }
}

fn head_of(terms: &[Term]) -> Vec<HeadArg> {
    terms.iter().cloned().map(HeadArg::Term).collect()
}

fn flip(op: CmpOp) -> CmpOp {
    match op {
        CmpOp::Eq => CmpOp::Ne,
        CmpOp::Ne => CmpOp::Eq,
        CmpOp::Lt => CmpOp::Ge,
        CmpOp::Le => CmpOp::Gt,
        CmpOp::Gt => CmpOp::Le,
        CmpOp::Ge => CmpOp::Lt,
    }
}

fn str_builtin(op: StrOp) -> (&'static str, &'static str) {
    match op {
        StrOp::StartsWith => ("starts_with", "STARTS WITH"),
        StrOp::EndsWith => ("ends_with", "ENDS WITH"),
        StrOp::Contains => ("contains", "CONTAINS"),
    }
}

/// Every conjunct of `a` joined with every conjunct of `b`, `a`-major.
fn product(a: &[Conj], b: &[Conj]) -> Vec<Conj> {
    let mut out = Vec::with_capacity(a.len().saturating_mul(b.len()));
    for x in a {
        for y in b {
            let mut c = x.clone();
            c.extend(y.iter().cloned());
            out.push(c);
        }
    }
    out
}

fn nnf(e: &Expr, neg: bool) -> Nnf<'_> {
    match e {
        Expr::Or { left, right, .. } | Expr::And { left, right, .. } => {
            let parts = vec![nnf(left, neg), nnf(right, neg)];
            if matches!(e, Expr::Or { .. }) != neg { Nnf::Or(parts) } else { Nnf::And(parts) }
        }
        Expr::Not { expr, .. } => nnf(expr, !neg),
        leaf => Nnf::Leaf(leaf, neg),
    }
}

fn nnf_count(n: &Nnf<'_>) -> u64 {
    match n {
        Nnf::Or(xs) => xs.iter().map(nnf_count).fold(0, u64::saturating_add),
        Nnf::And(xs) => xs.iter().map(nnf_count).fold(1, u64::saturating_mul),
        Nnf::Leaf(Expr::Pattern { pattern, .. }, false) => pattern_count(pattern),
        Nnf::Leaf(..) => 1,
    }
}

/// The alternatives a pattern lowers to: labels times directions times lengths. A
/// repetition the lowering refuses counts 1, so its own diagnostic is the one shown.
fn pattern_count(p: &PathPattern) -> u64 {
    let labels = |n: &NodePat| n.labels.len().max(1) as u64;
    let mut n = labels(&p.start);
    for (r, node) in &p.steps {
        let rel = match r.quant {
            None if r.dir == Dir::Undirected => 2,
            None => 1,
            Some(Quant { min: 0, max: None, .. }) => 2,
            Some(Quant { max: None, .. }) => 1,
            Some(Quant { min, max: Some(max), .. }) if max <= MAX_UNROLL => {
                u64::from(max - min) + 1
            }
            Some(_) => 1,
        };
        n = n.saturating_mul(rel).saturating_mul(labels(node));
    }
    n
}

fn is_bound(t: &Term, bound: &[String]) -> bool {
    match t {
        Term::Var { name, .. } => bound.contains(name),
        Term::Wild(_) => false,
        Term::Str { .. } | Term::Int { .. } => true,
    }
}

fn var_name(t: &Term) -> Option<&str> {
    match t {
        Term::Var { name, .. } => Some(name),
        _ => None,
    }
}

/// The variables `lits` bind: every positive atom's but a string test's or a
/// function call's, then `x = <bound>` to a fixpoint (analysis's safety rule).
fn bound_vars<'a, I>(lits: I, calls: &[String]) -> Vec<String>
where
    I: Iterator<Item = &'a Literal> + Clone,
{
    let mut out: Vec<String> = Vec::new();
    for lit in lits.clone() {
        if let Literal::Pos(a) = lit
            && !STRING_TESTS.contains(&a.pred.as_str())
            && !calls.contains(&a.pred)
        {
            for name in a.args.iter().filter_map(var_name) {
                if !out.iter().any(|o| o == name) {
                    out.push(name.to_string());
                }
            }
        }
    }
    loop {
        let mut changed = false;
        for lit in lits.clone() {
            if let Literal::Cmp(c) = lit
                && c.op == CmpOp::Eq
            {
                for (x, y) in [(&c.left, &c.right), (&c.right, &c.left)] {
                    if let Term::Var { name, .. } = x
                        && !out.contains(name)
                        && is_bound(y, &out)
                    {
                        out.push(name.clone());
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return out;
        }
    }
}

fn mentions(body: &[Literal], v: &str) -> bool {
    body.iter().any(|lit| match lit {
        Literal::Pos(a) | Literal::Neg(a) => a.args.iter().any(|t| var_name(t) == Some(v)),
        Literal::Cmp(c) => var_name(&c.left) == Some(v) || var_name(&c.right) == Some(v),
    })
}

/// Drop a literal that repeats an earlier one (an attribute read by WHERE and RETURN).
fn dedup_literals(body: &mut Conj) {
    let mut seen: Vec<String> = Vec::new();
    body.retain(|lit| {
        let key = lit.to_string();
        let fresh = !seen.contains(&key);
        if fresh {
            seen.push(key);
        }
        fresh
    });
}

/// A variable used once is `_` in a positive atom, and its assignment `x = <bound>`
/// is vacuous, so it goes: no singleton warning reaches a GQL author.
fn wild_singletons(r: &mut Rule) {
    let mut counts: Vec<(String, u32)> = Vec::new();
    let mut count = |name: &str| match counts.iter_mut().find(|(n, _)| n == name) {
        Some((_, c)) => *c += 1,
        None => counts.push((name.to_string(), 1)),
    };
    for arg in &r.head.args {
        match arg {
            HeadArg::Term(t) => var_name(t).into_iter().for_each(&mut count),
            HeadArg::Agg(a) => a.vars.iter().for_each(|v| count(v)),
        }
    }
    for lit in &r.body {
        match lit {
            Literal::Pos(a) | Literal::Neg(a) => {
                a.args.iter().filter_map(var_name).for_each(&mut count)
            }
            Literal::Cmp(c) => {
                [&c.left, &c.right].into_iter().filter_map(var_name).for_each(&mut count)
            }
        }
    }
    let once =
        |t: &Term| var_name(t).is_some_and(|n| counts.iter().any(|(m, c)| m == n && *c == 1));
    r.body.retain(|lit| match lit {
        Literal::Cmp(c) if c.op == CmpOp::Eq => !(once(&c.left) || once(&c.right)),
        _ => true,
    });
    for lit in &mut r.body {
        if let Literal::Pos(a) = lit {
            for t in &mut a.args {
                if once(t) {
                    *t = Term::Wild(t.span());
                }
            }
        }
    }
}

// ============================================================================
// The lowering
// ============================================================================

impl<'q> Lower<'q> {
    fn new(text: &'q str) -> Self {
        Lower {
            text,
            vars: Vec::new(),
            match_nodes: Vec::new(),
            node_vars: Vec::new(),
            paths: Vec::new(),
            attrs: Vec::new(),
            calls: Vec::new(),
            fresh: 0,
            helpers: Vec::new(),
            helper_preds: Vec::new(),
            hops: Vec::new(),
            counters: [0; 4],
            diags: Vec::new(),
        }
    }

    fn err(&mut self, span: Span, message: impl Into<String>) {
        if self.diags.len() < MAX_DIAGNOSTICS {
            self.diags.push(Diagnostic::error(span, SOURCE_NAME, None, message));
        }
    }

    fn kind(&self, name: &str) -> Option<VarKind> {
        self.vars.iter().find(|(n, _)| n == name).map(|(_, k)| *k)
    }

    fn fresh(&mut self) -> String {
        loop {
            let name = format!("_g{}", self.fresh);
            self.fresh += 1;
            if self.kind(&name).is_none() {
                return name;
            }
        }
    }

    fn helper(&mut self, h: Helper) -> String {
        let (i, word) = match h {
            Helper::Hop => (0, "hop"),
            Helper::Not => (1, "not"),
            Helper::In => (2, "in"),
            Helper::Match => (3, "match"),
        };
        let name = format!("{PREFIX}{word}{}", self.counters[i]);
        self.counters[i] += 1;
        self.helper_preds.push(name.clone());
        name
    }

    fn slice(&self, span: Span) -> String {
        self.text.get(span.start as usize..span.end as usize).unwrap_or_default().to_string()
    }

    /// A property key or function name must not read the lowering's own predicates.
    fn allowed(&mut self, id: &Ident, what: &str) -> bool {
        if id.name == OUTPUT || id.name.starts_with(PREFIX) {
            let message = format!(
                "`{}` is reserved for the lowering (`{OUTPUT}` and `{PREFIX}*` helpers): rename the {what}",
                id.name
            );
            self.err(id.span, message);
            return false;
        }
        true
    }

    fn lit_term(&mut self, l: &Lit) -> Option<Term> {
        match l {
            Lit::Str { value, span } => Some(str_at(value, *span)),
            Lit::Int { value, span } => Some(Term::Int { value: *value, span: *span }),
            Lit::Bool { span, .. } => {
                self.err(*span, BOOL_LITERAL);
                None
            }
        }
    }

    /// Prepend `node(v, _)` / `category(v)` for each of `must` the body leaves unbound.
    fn bind(&self, body: &mut Conj, must: &[(String, Bind)]) {
        let mut front: Conj = Vec::new();
        for (v, how) in must {
            if !bound_vars(front.iter().chain(body.iter()), &self.calls).contains(v) {
                front.push(match how {
                    Bind::Node => node_any(v, Span::default()),
                    Bind::Cat => Literal::Pos(Atom::new("category", vec![Term::var(v)])),
                });
            }
        }
        if !front.is_empty() {
            front.append(body);
            *body = front;
        }
    }

    /// Range restriction: `required`, then every other node variable the body names.
    fn restrict(&self, body: &mut Conj, required: &[String]) {
        let mut must: Vec<(String, Bind)> =
            required.iter().map(|v| (v.clone(), Bind::Node)).collect();
        for v in &self.node_vars {
            if !required.contains(v) && mentions(body, v) {
                must.push((v.clone(), Bind::Node));
            }
        }
        self.bind(body, &must);
        dedup_literals(body);
    }

    // ------------------------------------------------------------------------
    // The query
    // ------------------------------------------------------------------------

    fn query(&mut self, q: &Query) -> Option<(Compiled, usize)> {
        self.declare(q);
        if !self.diags.is_empty() {
            return None;
        }
        let cond = q.condition.as_ref().map(|c| nnf(c, false));
        let cond_n = cond.as_ref().map_or(1, nnf_count);
        let patterns = q.matches.iter().flat_map(|m| &m.patterns);
        let total = patterns.clone().map(pattern_count).fold(cond_n, u64::saturating_mul);
        if total > MAX_ALTERNATIVES {
            let span = match &q.condition {
                Some(c) if cond_n > 1 => c.span(),
                _ => q.span,
            };
            self.err(
                span,
                format!(
                    "the query expands to {total} alternatives (limit {MAX_ALTERNATIVES}): split it"
                ),
            );
            return None;
        }
        let mut alts: Vec<Conj> = vec![Vec::new()];
        for p in patterns {
            let lowered = self.pattern(p, &mut Scope::Match);
            alts = product(&alts, &lowered);
        }
        if let Some(c) = &cond {
            let lowered = self.expand(c);
            alts = product(&alts, &lowered);
        }
        let (head, pre, columns, path_column) = self.ret(q);
        let order_by = self.order(q);
        if !self.diags.is_empty() {
            return None;
        }
        let required = self.match_nodes.clone();
        let mut bodies: Vec<Conj> = Vec::new();
        let mut seen: Vec<String> = Vec::new();
        for alt in product(&alts, &[pre]) {
            let mut body = alt;
            self.restrict(&mut body, &required);
            let key: Vec<String> = body.iter().map(ToString::to_string).collect();
            let key = key.join(", ");
            if !seen.contains(&key) {
                seen.push(key);
                bodies.push(body);
            }
        }
        let alternatives = bodies.len();
        let mut rules = Vec::new();
        if bodies.len() > 1 && head.iter().any(|a| matches!(a, HeadArg::Agg(_))) {
            let mut vars: Vec<String> = Vec::new();
            for arg in &head {
                let names: Vec<&str> = match arg {
                    HeadArg::Term(t) => var_name(t).into_iter().collect(),
                    HeadArg::Agg(a) => a.vars.iter().map(String::as_str).collect(),
                };
                for n in names {
                    if !vars.iter().any(|v| v == n) {
                        vars.push(n.to_string());
                    }
                }
            }
            let pred = self.helper(Helper::Match);
            let args: Vec<Term> = vars.iter().map(Term::var).collect();
            for body in bodies {
                self.helpers.push(rule(&pred, head_of(&args), body, q.span));
            }
            rules.push(rule(OUTPUT, head, vec![Literal::Pos(atom(&pred, args, q.span))], q.span));
        } else {
            for body in bodies {
                rules.push(rule(OUTPUT, head.clone(), body, q.span));
            }
        }
        rules.append(&mut self.helpers);
        rules.iter_mut().for_each(wild_singletons);
        let mut program = Program::new();
        program.add_source(SOURCE_NAME);
        program.rules = rules;
        let compiled = Compiled {
            program,
            output: OUTPUT.to_string(),
            columns,
            order_by,
            limit: q.limit,
            path_column,
        };
        Some((compiled, alternatives))
    }

    /// Record every MATCH variable's kind; a name bound twice as a relationship or
    /// path, or as two kinds, is a diagnostic. A path variable needs exactly one
    /// variable-length relationship.
    fn declare(&mut self, q: &Query) {
        for p in q.matches.iter().flat_map(|m| &m.patterns) {
            if let Some(v) = &p.var {
                self.declare_var(v, VarKind::Path);
                let quants: Vec<Span> =
                    p.steps.iter().filter_map(|(r, _)| r.quant.map(|q| q.span)).collect();
                if quants.len() != 1 {
                    let span = quants.get(1).copied().unwrap_or(p.span);
                    let message = format!(
                        "path variable `{}` needs exactly one variable-length relationship in its pattern (found {})",
                        v.name,
                        quants.len()
                    );
                    self.err(span, message);
                }
            }
            let nodes = std::iter::once(&p.start).chain(p.steps.iter().map(|(_, n)| n));
            for n in nodes {
                if let Some(v) = &n.var {
                    self.declare_var(v, VarKind::Node);
                }
            }
            for (r, _) in &p.steps {
                if let Some(v) = &r.var {
                    let kind = if r.quant.is_some() { VarKind::QuantRel } else { VarKind::Rel };
                    self.declare_var(v, kind);
                }
            }
        }
    }

    fn declare_var(&mut self, v: &Ident, kind: VarKind) {
        match self.kind(&v.name) {
            None => self.vars.push((v.name.clone(), kind)),
            Some(VarKind::Node) if kind == VarKind::Node => {}
            Some(old) => {
                let message = if kind == VarKind::Node {
                    format!("`{}` is {}, not a node", v.name, old.describe())
                } else {
                    format!(
                        "`{}` is already {}: a relationship or path variable names one pattern",
                        v.name,
                        old.describe()
                    )
                };
                self.err(v.span, message);
            }
        }
    }

    // ------------------------------------------------------------------------
    // Patterns
    // ------------------------------------------------------------------------

    fn pattern(&mut self, p: &PathPattern, scope: &mut Scope) -> Vec<Conj> {
        let first = self.node_var(&p.start, scope);
        let mut alts = self.node(&p.start, &first);
        let mut prev = first.clone();
        for (r, n) in &p.steps {
            let next = self.node_var(n, scope);
            let cat = self.rel_var(r, scope);
            let rel = self.step(r, &prev, &next, cat.as_deref());
            alts = product(&alts, &rel);
            let node = self.node(n, &next);
            alts = product(&alts, &node);
            prev = next;
        }
        if p.steps.is_empty() {
            for conj in alts.iter_mut().filter(|c| c.is_empty()) {
                conj.push(node_any(&first, p.start.span));
            }
        }
        if let (Scope::Match, Some(v)) = (&*scope, &p.var) {
            self.paths.push(PathVar { name: v.name.clone(), start: first, end: prev });
        }
        alts
    }

    fn node_var(&mut self, n: &NodePat, scope: &mut Scope) -> String {
        let name = match (&n.var, &mut *scope) {
            (None, _) => self.fresh(),
            (Some(id), Scope::Match) => id.name.clone(),
            (Some(id), Scope::Local { renames, shared }) => match self.kind(&id.name) {
                Some(VarKind::Node) => {
                    if !shared.contains(&id.name) {
                        shared.push(id.name.clone());
                    }
                    id.name.clone()
                }
                Some(k) => {
                    self.err(id.span, format!("`{}` is {}, not a node", id.name, k.describe()));
                    id.name.clone()
                }
                None => match renames.iter().find(|(u, _)| *u == id.name) {
                    Some((_, f)) => f.clone(),
                    None => {
                        let f = self.fresh();
                        renames.push((id.name.clone(), f.clone()));
                        f
                    }
                },
            },
        };
        if matches!(scope, Scope::Match) && !self.match_nodes.contains(&name) {
            self.match_nodes.push(name.clone());
        }
        if !self.node_vars.contains(&name) {
            self.node_vars.push(name.clone());
        }
        name
    }

    /// The category variable a relationship binds: its variable when unquantified
    /// (renamed fresh in a WHERE pattern), else none.
    fn rel_var(&mut self, r: &RelPat, scope: &mut Scope) -> Option<String> {
        let id = r.var.as_ref().filter(|_| r.quant.is_none())?;
        match scope {
            Scope::Match => Some(id.name.clone()),
            Scope::Local { renames, .. } => {
                if let Some(k) = self.kind(&id.name) {
                    let message = format!(
                        "`{}` is already {}: a WHERE pattern cannot rebind it",
                        id.name,
                        k.describe()
                    );
                    self.err(id.span, message);
                    return None;
                }
                if let Some((_, f)) = renames.iter().find(|(u, _)| *u == id.name) {
                    return Some(f.clone());
                }
                let f = self.fresh();
                renames.push((id.name.clone(), f.clone()));
                Some(f)
            }
        }
    }

    fn node(&mut self, n: &NodePat, v: &str) -> Vec<Conj> {
        let mut alts: Vec<Conj> = if n.labels.is_empty() {
            vec![Vec::new()]
        } else {
            let label = |l: &Ident| {
                vec![Literal::Pos(atom(
                    "node",
                    vec![Term::var(v), str_at(&l.name, l.span)],
                    n.span,
                ))]
            };
            n.labels.iter().map(label).collect()
        };
        for (k, lit) in &n.props {
            if !self.allowed(k, "property") {
                continue;
            }
            let Some(t) = self.lit_term(lit) else { continue };
            let a = Literal::Pos(atom(&k.name, vec![Term::var(v), t], k.span));
            alts.iter_mut().for_each(|c| c.push(a.clone()));
        }
        alts
    }

    fn step(&mut self, r: &RelPat, left: &str, right: &str, cat: Option<&str>) -> Vec<Conj> {
        let (src, dst) = if r.dir == Dir::In { (right, left) } else { (left, right) };
        let Some(q) = r.quant else {
            let mut alts = vec![self.hop(r, src, dst, cat, false)];
            if r.dir == Dir::Undirected {
                alts.push(self.hop(r, dst, src, cat, false));
            }
            return alts;
        };
        self.repeat(r, q, src, dst)
    }

    /// One hop `x -> y` of `r` (`both`: either way, through a two-way helper).
    fn hop(&mut self, r: &RelPat, x: &str, y: &str, cat: Option<&str>, both: bool) -> Conj {
        match (r.types.as_slice(), both) {
            ([], false) => {
                let c = cat.map_or_else(Term::wild, Term::var);
                vec![Literal::Pos(atom("edge", vec![Term::var(x), Term::var(y), c], r.span))]
            }
            ([t], false) => {
                let mut conj = vec![Literal::Pos(atom(
                    "edge",
                    vec![Term::var(x), Term::var(y), str_at(&t.name, t.span)],
                    r.span,
                ))];
                if let Some(c) = cat {
                    conj.push(eq(Term::var(c), str_at(&t.name, t.span), r.span));
                }
                conj
            }
            _ => {
                let pred = self.hop_helper(r, cat.is_some(), both);
                let mut args = vec![Term::var(x), Term::var(y)];
                args.extend(cat.map(Term::var));
                vec![Literal::Pos(atom(&pred, args, r.span))]
            }
        }
    }

    /// `gql_hop<n>(x, y[, c])`: one rule per type (and per direction when `both`),
    /// shared by every relationship with the same types and shape.
    fn hop_helper(&mut self, r: &RelPat, with_cat: bool, both: bool) -> String {
        let names: Vec<&str> = r.types.iter().map(|t| t.name.as_str()).collect();
        let key = format!("{}/{with_cat}/{both}", names.join("|"));
        if let Some((_, pred)) = self.hops.iter().find(|(k, _)| *k == key) {
            return pred.clone();
        }
        let pred = self.helper(Helper::Hop);
        self.hops.push((key, pred.clone()));
        let mut head = vec![Term::var("x"), Term::var("y")];
        if with_cat {
            head.push(Term::var("c"));
        }
        let dirs: &[(&str, &str)] = if both { &[("x", "y"), ("y", "x")] } else { &[("x", "y")] };
        let types: Vec<Option<&Ident>> =
            if r.types.is_empty() { vec![None] } else { r.types.iter().map(Some).collect() };
        for t in &types {
            for (a, b) in dirs {
                let ty = match t {
                    Some(t) => str_at(&t.name, t.span),
                    None if with_cat => Term::var("c"),
                    None => Term::wild(),
                };
                let mut body = vec![Literal::Pos(atom(
                    "edge",
                    vec![Term::var(*a), Term::var(*b), ty],
                    r.span,
                ))];
                if let (Some(t), true) = (t, with_cat) {
                    body.push(eq(Term::var("c"), str_at(&t.name, t.span), r.span));
                }
                self.helpers.push(rule(&pred, head_of(&head), body, r.span));
            }
        }
        pred
    }

    /// A quantified relationship `src ->{m,n} dst` (module doc).
    fn repeat(&mut self, r: &RelPat, q: Quant, src: &str, dst: &str) -> Vec<Conj> {
        let same = || vec![eq(Term::var(src), Term::var(dst), q.span)];
        let mut alts = Vec::new();
        let Some(max) = q.max else {
            if r.dir == Dir::Undirected {
                self.err(
                    r.span,
                    "a variable-length relationship needs a direction (`->` or `<-`): reachability follows edges one way",
                );
                return alts;
            }
            if q.min > MAX_UNROLL + 1 {
                let message = format!(
                    "a lower bound above {} hops unrolls more than {MAX_UNROLL}: lower it, or use * (any length)",
                    MAX_UNROLL + 1
                );
                self.err(q.span, message);
                return alts;
            }
            if q.min == 0 {
                alts.push(same());
            }
            let mut conj = Vec::new();
            let mut at = src.to_string();
            for _ in 1..q.min.max(1) {
                let next = self.fresh();
                conj.extend(self.hop(r, &at, &next, None, false));
                at = next;
            }
            let cats = if r.types.is_empty() {
                "*".to_string()
            } else {
                r.types.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join("|")
            };
            let reach = vec![Term::var(&at), Term::var(dst), str_at(&cats, r.span)];
            conj.push(Literal::Pos(atom("reaches", reach, r.span)));
            alts.push(conj);
            return alts;
        };
        if max > MAX_UNROLL {
            self.err(
                q.span,
                format!("bounded repetition above {MAX_UNROLL} hops: use * (any length)"),
            );
            return alts;
        }
        let both = r.dir == Dir::Undirected;
        for k in q.min..=max {
            if k == 0 {
                alts.push(same());
                continue;
            }
            let mut conj = Vec::new();
            let mut at = src.to_string();
            for i in 1..=k {
                let next = if i == k { dst.to_string() } else { self.fresh() };
                conj.extend(self.hop(r, &at, &next, None, both));
                at = next;
            }
            alts.push(conj);
        }
        alts
    }

    // ------------------------------------------------------------------------
    // WHERE
    // ------------------------------------------------------------------------

    fn expand(&mut self, n: &Nnf<'_>) -> Vec<Conj> {
        match n {
            Nnf::Or(xs) => {
                let mut out = Vec::new();
                for x in xs {
                    out.extend(self.expand(x));
                }
                out
            }
            Nnf::And(xs) => {
                let mut out = vec![Vec::new()];
                for x in xs {
                    let e = self.expand(x);
                    out = product(&out, &e);
                }
                out
            }
            Nnf::Leaf(e, neg) => self.leaf(e, *neg),
        }
    }

    fn leaf(&mut self, e: &Expr, neg: bool) -> Vec<Conj> {
        match e {
            Expr::Cmp { op, left, right, span } => {
                let (Some(l), Some(r)) = (self.operand(left), self.operand(right)) else {
                    return Vec::new();
                };
                let op = if neg { flip(*op) } else { *op };
                let mut conj = l.pre;
                conj.extend(r.pre);
                conj.push(Literal::Cmp(Comparison {
                    op,
                    left: l.term,
                    right: r.term,
                    span: *span,
                }));
                vec![conj]
            }
            Expr::StrTest { op, left, right, span } => {
                let (builtin, word) = str_builtin(*op);
                for o in [left, right] {
                    if let Operand::Var(id) = o
                        && self.kind(&id.name) == Some(VarKind::Node)
                    {
                        self.err(
                            id.span,
                            format!(
                                "{word} compares strings: test an attribute, `{}.<key>`",
                                id.name
                            ),
                        );
                        return Vec::new();
                    }
                }
                let (Some(l), Some(r)) = (self.operand(left), self.operand(right)) else {
                    return Vec::new();
                };
                let lit = Literal::Pos(atom(builtin, vec![l.term.clone(), r.term.clone()], *span));
                self.test(lit, vec![l, r], neg, *span)
            }
            Expr::Call { name, args, span } => {
                if !self.allowed(name, "function") {
                    return Vec::new();
                }
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.operand(a));
                }
                let Some(vals) = vals.into_iter().collect::<Option<Vec<Val>>>() else {
                    return Vec::new();
                };
                if !self.calls.contains(&name.name) {
                    self.calls.push(name.name.clone());
                }
                let terms = vals.iter().map(|v| v.term.clone()).collect();
                self.test(Literal::Pos(atom(&name.name, terms, *span)), vals, neg, *span)
            }
            Expr::In { left, list, span } => {
                let Some(l) = self.operand(left) else { return Vec::new() };
                let Some(pred) = self.in_table(list) else { return Vec::new() };
                let a = atom(&pred, vec![l.term], *span);
                let mut conj = l.pre;
                conj.push(if neg { Literal::Neg(a) } else { Literal::Pos(a) });
                vec![conj]
            }
            Expr::Pattern { pattern, span, .. } => self.pattern_condition(pattern, neg, *span),
            Expr::Or { .. } | Expr::And { .. } | Expr::Not { .. } => {
                let n = nnf(e, neg);
                self.expand(&n)
            }
        }
    }

    /// A pattern predicate: inlined when positive, else `!gql_not<n>(shared)` with one
    /// helper rule per alternative of the pattern.
    fn pattern_condition(&mut self, p: &PathPattern, neg: bool, span: Span) -> Vec<Conj> {
        let n = pattern_count(p);
        if neg && n > MAX_ALTERNATIVES {
            self.err(
                p.span,
                format!(
                    "the query expands to {n} alternatives (limit {MAX_ALTERNATIVES}): split it"
                ),
            );
            return Vec::new();
        }
        let mut scope = Scope::Local { renames: Vec::new(), shared: Vec::new() };
        let alts = self.pattern(p, &mut scope);
        if !neg {
            return alts;
        }
        let Scope::Local { shared, .. } = scope else { return Vec::new() };
        let pred = self.helper(Helper::Not);
        let args: Vec<Term> = shared.iter().map(Term::var).collect();
        for mut body in alts {
            self.restrict(&mut body, &shared);
            self.helpers.push(rule(&pred, head_of(&args), body, span));
        }
        vec![vec![Literal::Neg(atom(&pred, args, span))]]
    }

    /// A string test or function call `lit` over `vals`: in place when positive, else
    /// the negated helper `gql_not<n>(shared) :- <bindings>, lit.`.
    fn test(&mut self, lit: Literal, vals: Vec<Val>, neg: bool, span: Span) -> Vec<Conj> {
        let mut pre: Conj = Vec::new();
        let mut owns: Vec<(String, Option<Bind>)> = Vec::new();
        for v in vals {
            pre.extend(v.pre);
            for o in v.owns {
                if !owns.iter().any(|(n, _)| *n == o.0) {
                    owns.push(o);
                }
            }
        }
        if !neg {
            pre.push(lit);
            return vec![pre];
        }
        let pred = self.helper(Helper::Not);
        let args: Vec<Term> = owns.iter().map(|(n, _)| Term::var(n)).collect();
        let must: Vec<(String, Bind)> =
            owns.iter().filter_map(|(n, b)| b.map(|b| (n.clone(), b))).collect();
        let mut body = pre.clone();
        self.bind(&mut body, &must);
        body.push(lit);
        dedup_literals(&mut body);
        self.helpers.push(rule(&pred, head_of(&args), body, span));
        pre.push(Literal::Neg(atom(&pred, args, span)));
        vec![pre]
    }

    /// `IN [..]`'s fact table `gql_in<n>(lit).`, one fact per list entry.
    fn in_table(&mut self, list: &[Lit]) -> Option<String> {
        let mut terms = Vec::with_capacity(list.len());
        for l in list {
            terms.push(self.lit_term(l)?);
        }
        let pred = self.helper(Helper::In);
        for t in terms {
            let span = t.span();
            self.helpers.push(rule(&pred, vec![HeadArg::Term(t)], Vec::new(), span));
        }
        Some(pred)
    }

    fn operand(&mut self, o: &Operand) -> Option<Val> {
        match o {
            Operand::Var(id) => {
                self.node_ref(id)?;
                let owns = vec![(id.name.clone(), Some(Bind::Node))];
                Some(Val {
                    term: Term::Var { name: id.name.clone(), span: id.span },
                    pre: Vec::new(),
                    owns,
                })
            }
            Operand::Prop { var, key, span } => {
                self.node_ref(var)?;
                if !self.allowed(key, "property") {
                    return None;
                }
                let x = self.attr_var(&var.name, &key.name);
                let read = atom(&key.name, vec![Term::var(&var.name), Term::var(&x)], *span);
                let owns = vec![(var.name.clone(), Some(Bind::Node)), (x.clone(), None)];
                Some(Val { term: Term::var(x), pre: vec![Literal::Pos(read)], owns })
            }
            Operand::Lit(l) => {
                Some(Val { term: self.lit_term(l)?, pre: Vec::new(), owns: Vec::new() })
            }
            Operand::Type { var, .. } => {
                let message = match self.kind(&var.name) {
                    Some(VarKind::Rel) => {
                        let owns = vec![(var.name.clone(), Some(Bind::Cat))];
                        let term = Term::Var { name: var.name.clone(), span: var.span };
                        return Some(Val { term, pre: Vec::new(), owns });
                    }
                    Some(VarKind::QuantRel) => {
                        format!(
                            "`{}` names a variable-length relationship, which has no single type",
                            var.name
                        )
                    }
                    Some(k) => format!(
                        "type() takes a relationship variable; `{}` is {}",
                        var.name,
                        k.describe()
                    ),
                    None => format!("`{}` is not bound by MATCH", var.name),
                };
                self.err(var.span, message);
                None
            }
        }
    }

    /// `id` must be a MATCH node variable.
    fn node_ref(&mut self, id: &Ident) -> Option<()> {
        let message = match self.kind(&id.name) {
            Some(VarKind::Node) => return Some(()),
            Some(VarKind::Rel) => {
                format!("`{0}` is a relationship variable: use type({0})", id.name)
            }
            Some(VarKind::QuantRel) => {
                format!(
                    "`{}` names a variable-length relationship, which has no single type",
                    id.name
                )
            }
            Some(VarKind::Path) => {
                format!("`{}` is a path variable: it is a RETURN column only", id.name)
            }
            None => format!("`{}` is not bound by MATCH", id.name),
        };
        self.err(id.span, message);
        None
    }

    /// The value variable of `var.key`: one per attribute read, shared by WHERE and
    /// RETURN.
    fn attr_var(&mut self, var: &str, key: &str) -> String {
        if let Some((_, _, x)) = self.attrs.iter().find(|(v, k, _)| v == var && k == key) {
            return x.clone();
        }
        let x = self.fresh();
        self.attrs.push((var.to_string(), key.to_string(), x.clone()));
        x
    }

    // ------------------------------------------------------------------------
    // RETURN, ORDER BY
    // ------------------------------------------------------------------------

    /// The head, the attribute atoms its items read, the columns and the path column.
    fn ret(&mut self, q: &Query) -> (Vec<HeadArg>, Conj, Vec<String>, Option<usize>) {
        let mut head = Vec::with_capacity(q.items.len() + 1);
        let mut pre = Vec::new();
        let mut columns = Vec::with_capacity(q.items.len());
        let mut path_column = None;
        let mut trailing = None;
        for (i, item) in q.items.iter().enumerate() {
            let span = match &item.expr {
                RetExpr::Operand(o) => o.span(),
                RetExpr::Agg(a) => a.span,
            };
            columns.push(match &item.alias {
                Some(a) => a.name.clone(),
                None => self.slice(span),
            });
            match &item.expr {
                RetExpr::Operand(Operand::Var(id))
                    if self.kind(&id.name) == Some(VarKind::Path) =>
                {
                    if path_column.is_some() {
                        self.err(id.span, "a query returns at most one path variable");
                        continue;
                    }
                    let Some(p) = self.paths.iter().find(|p| p.name == id.name) else { continue };
                    head.push(HeadArg::Term(Term::Var { name: p.start.clone(), span: id.span }));
                    trailing = Some(Term::Var { name: p.end.clone(), span: id.span });
                    path_column = Some(i);
                }
                RetExpr::Operand(o) => {
                    if let Some(v) = self.operand(o) {
                        pre.extend(v.pre);
                        head.push(HeadArg::Term(v.term));
                    }
                }
                RetExpr::Agg(a) => {
                    let vars = match &a.arg {
                        None => {
                            let mut all = self.match_nodes.clone();
                            all.extend(
                                self.vars
                                    .iter()
                                    .filter(|(_, k)| *k == VarKind::Rel)
                                    .map(|(n, _)| n.clone()),
                            );
                            all
                        }
                        Some(Operand::Lit(l)) => {
                            self.err(
                                l.span(),
                                "an aggregate takes a variable, `var.key` or type(r)",
                            );
                            continue;
                        }
                        Some(o) => match self.operand(o) {
                            Some(Val { term: Term::Var { name, .. }, pre: p, .. }) => {
                                pre.extend(p);
                                vec![name]
                            }
                            _ => continue,
                        },
                    };
                    head.push(HeadArg::Agg(Aggregate { op: a.op, vars, span: a.span }));
                }
            }
        }
        head.extend(trailing.map(HeadArg::Term));
        (head, pre, columns, path_column)
    }

    /// Each key is a column: an alias, else an item written the same way.
    fn order(&mut self, q: &Query) -> Vec<OrderKey> {
        let mut out = Vec::with_capacity(q.order_by.len());
        for k in &q.order_by {
            let alias = q.items.iter().position(|it| match (&k.key, &it.alias) {
                (Operand::Var(id), Some(a)) => a.name == id.name,
                _ => false,
            });
            let written = || {
                q.items.iter().position(|it| match (&k.key, &it.expr) {
                    (Operand::Var(a), RetExpr::Operand(Operand::Var(b))) => a.name == b.name,
                    (
                        Operand::Prop { var: a, key: x, .. },
                        RetExpr::Operand(Operand::Prop { var: b, key: y, .. }),
                    ) => a.name == b.name && x.name == y.name,
                    _ => false,
                })
            };
            match alias.or_else(written) {
                Some(column) => out.push(OrderKey { column, descending: k.descending }),
                None => {
                    let span = k.key.span();
                    let message = format!(
                        "ORDER BY `{}` names no RETURN column: order by a returned item or its alias",
                        self.slice(span)
                    );
                    self.err(span, message);
                }
            }
        }
        out
    }
}
