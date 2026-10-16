//! `query` / `validate` (DB.2b): Datalog text over the code graph, as an answer.
//!
//! **Pipeline.** [`query`] parses the text (glia-rules' parser, spans tagged with
//! `QueryOptions::source_name`, default `<query>`), merges in the prelude rules it
//! reads (DB.2c, `prelude::merge`: the predicate prelude, pruned to what the program
//! and its asked outputs read), analyses it against the code schema and evaluates
//! it over `prelude::PreludeFacts` (the [`CodeFacts`] plus the engine relations)
//! within the row budget. The schema is the catalog of that source
//! (`glia_rules::source::catalog_of`): the core predicates (`node/2`, `edge/3`,
//! `edge_conf/4`, `cell/3`, `edge_cell/5`, `kind/1`, `category/1`,
//! `carry_category/1`), the code attributes
//! ([`CODE_ATTRIBUTES`](super::CODE_ATTRIBUTES)) and the builtins (`reaches`, `scc`,
//! the string tests, `in_scope/2`, `under/2`, and the engine relations
//! `dependency_category/1`, `symbol_kind/1`, `effect_sink/3`, `live/1`,
//! `module_import/2`). A rule whose head names any of them is refused by analysis,
//! as is an unknown node kind / edge category / cell type / predicate, an arity
//! mismatch, an unsafe variable or an unstratifiable negation; the messages are
//! glia-rules' table (`analysis::msg`) verbatim, so this layer adds no second
//! wording. The one message of its own is the merge's: a rule whose head names a
//! prelude predicate or engine relation, ``` `carry` is a prelude predicate
//! (docs/rules.md); name yours differently ```. [`validate`] runs the same parse,
//! merge and analysis with no graph (the schema is static). [`evaluate_rule`] is the
//! pipeline over a caller's facts for one output, every row explained (check's rule
//! invariants).
//!
//! **Outputs.** `QueryOptions::outputs` (`name` or `name/arity`; a prelude
//! predicate may be asked for), else the user program's sinks: every predicate it
//! defines that no rule of ANOTHER predicate reads (glia-rules' default rule,
//! computed before the prelude joins, so no prelude predicate is ever a default
//! output; a helper like `called` in `uncalled(f) :- ..., !called(f).` is not an
//! output and the magic rewrite can still seed a recursive helper from its bound
//! callers; outputs are never rewritten). Asked-for outputs are reported in the
//! order asked, the sinks in first-definition order. An output no rule defines is an
//! analysis error.
//!
//! **Rows.** A node value is rendered as qname + kind + 1-based file:line through
//! the facts' ONE Locator (LD.1); strings and integers as they are. A row's `file`
//! / `line` are its first node value's. Rows of an output sort by their rendered
//! value tuples, column by column: Int < Str < Node, a node by qname, then file,
//! then line; ties keep glia-rules' order (deterministic). NodeIds never order
//! anything: they hash a path-derived RepoId and differ between checkouts. The
//! sorted rows are then cut to `limit`; `QueryOutput::total` keeps the count.
//!
//! **Tiers and caveats (Q5).** A row's tier is glia-rules' lattice (the min over
//! its derivations of the max over their inputs), named as `why` names it. A row
//! whose derivation goes through a negation carries the coverage caveats
//! (`coverage::caveats_for`) of every edge category the negated sub-program reads,
//! glia-rules' static, conservative cone (`eval::AbsenceCaveat`): a category read
//! through a variable, `_` or `"*"` reads as every category. An engine relation the
//! negated sub-program reaches adds the categories it is computed from
//! (`prelude::relation_reads`: `!live(n)` reads every carry category), which
//! glia-rules cannot see in a domain builtin. Positive atoms tier a row; negation
//! only adds caveats.
//!
//! **Witnesses.** With `explain`, each shown row's minimal-height derivation
//! (glia-rules' `Evaluation::explain_all`, one closure index for the answer) gives
//! its leaf edges, depth-first and left to right; each is the edge at its global
//! ordinal in `all_edges` order (the first edge with that key at the row's level),
//! located through `facts::located_edge` with `why::tier_of`'s tier and note, and
//! the list is capped at `check::MAX_EVIDENCE`.
//!
//! **Absence (LD.8a).** When no output has a row, `absence` is
//! `absence::empty(merged, "query", <the text's first 120 chars>, "no_match", "no
//! row of <pred>/<n> in this graph", <every category the outputs' rules read,
//! positive or negated>, None)`.
//!
//! **Errors** ([`RuleError`]): `parse`, `analysis`, `budget` (the row budget, or
//! an explain's candidate cap) and `evaluation` (a builtin or run-time type
//! error). `rendered` is glia-rules' text (`<source>:<line>:<col>: error: ...`); a
//! budget stop adds the surface's way to raise the budget. Nothing is ever
//! truncated silently.
//!
//! **Markers**, on stderr: glia-rules' `[rules] eval` / `[rules] closure` / `[rules]
//! budget` lines, the prelude's `[rules] prelude relations=<n> (...)` once per
//! evaluation, then, once per answered query, `[query] lang=<datalog|gql>
//! outputs=<o> rows=<r> fact=<f> derived=<d> heuristic=<h> caveated=<c>
//! surface=<cli|py|engine>`, counted over the rows returned. Analysis warnings
//! (singleton variables, unseeded recursion) print as glia-rules renders them,
//! `<source>:<line>:<col>: warning: ...`.
//!
//! The evaluator reads structure only: nodes, kinds, locations, edges and cells
//! (SECURITY.md); docs/rules.md is the language reference.

use std::collections::{BTreeMap, HashMap};

use glia_activation::algo::GraphSource;
use glia_code_domain::evidence::Evidence;
use glia_core::{Edge, NodeId};
use glia_graph::MergedGraph;
use glia_rules::analysis::analyse;
use glia_rules::diag::{Diagnostic, RulesError};
use glia_rules::eval::{AbsenceCaveat, Options, Row, evaluate};
use glia_rules::ir::{HeadArg, Literal, Program, Term};
use glia_rules::parser::parse_program;
use glia_rules::source::catalog_of;
use glia_rules::value::{Tier, Value};
use serde::Serialize;

use super::facts::{CodeFacts, located_edge, tier_name};
use super::prelude::{self, PreludeFacts};
use crate::absence::{self, Absence};
use crate::check::{MAX_EVIDENCE, ViolationEdge};
use crate::coverage::{CoverageNote, caveats_for};
use crate::profile::CODE_PROFILE;
use crate::why;

/// The source name a query's spans carry when the caller names none.
const DEFAULT_SOURCE: &str = "<query>";

/// How many chars of the program text an absence quotes as its query.
const ABSENCE_QUERY_CHARS: usize = 120;

/// How [`query`] runs. Built with `Default` and field assignment outside the crate.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct QueryOptions {
    /// The predicates answered, as `name` or `name/arity`; empty: the program's
    /// sinks (module doc).
    pub outputs: Vec<String>,
    /// Rows shown per output, cut after the sort; `None` shows every row.
    pub limit: Option<usize>,
    /// Build each shown row's witness edges.
    pub explain: bool,
    /// The row budget; `None` is glia-rules' `DEFAULT_MAX_ROWS`.
    pub row_budget: Option<usize>,
    /// The name diagnostics and spans carry (`<query>` when empty).
    pub source_name: String,
    /// Who asked: `cli`, `py` or `engine` (the marker's `surface=` and the budget
    /// hint).
    pub surface: &'static str,
}

impl Default for QueryOptions {
    fn default() -> Self {
        QueryOptions {
            outputs: Vec::new(),
            limit: None,
            explain: false,
            row_budget: None,
            source_name: DEFAULT_SOURCE.to_string(),
            surface: "engine",
        }
    }
}

/// A query's answer: its outputs, their rows in output order, and, exactly when no
/// output has a row, why.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone)]
pub struct QueryAnswer {
    /// `datalog` | `gql`.
    pub language: &'static str,
    pub outputs: Vec<QueryOutput>,
    /// Every output's shown rows, output by output, each output's in rendered order.
    pub results: Vec<QueryRow>,
    /// `Some` iff no output has a row (every `total` is 0). A `limit` of 0 shows no
    /// row of a non-empty answer and carries no absence: the totals say why.
    pub absence: Option<Absence>,
}

/// One output predicate of an answer.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct QueryOutput {
    /// The predicate's name (`x`, not `x/2`).
    pub predicate: String,
    pub arity: usize,
    /// The head variable names of the predicate's first rule, in source order; a
    /// constant position is `c<i>` (0-based), an aggregate `op(vars)`.
    pub columns: Vec<String>,
    /// Rows the output has.
    pub total: usize,
    /// Rows returned (`total` cut to `limit`).
    pub shown: usize,
}

/// One answer row.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone)]
pub struct QueryRow {
    /// The output predicate's name.
    pub predicate: String,
    pub values: Vec<QueryValue>,
    /// `fact` | `derived` | `heuristic`.
    pub tier: &'static str,
    /// The first node value's location (1-based line).
    pub file: Option<String>,
    pub line: Option<i64>,
    /// The coverage caveats of the categories the row's negations read (Q5).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub caveats: Vec<CoverageNote>,
    /// With `explain`: the derivation's leaf edges, located, at most
    /// `check::MAX_EVIDENCE`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub witness: Vec<ViolationEdge>,
}

/// One rendered value. JSON: `{"node": {..}}`, `{"str": ".."}`, `{"int": n}`.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum QueryValue {
    Node {
        qname: String,
        kind: &'static str,
        file: Option<String>,
        /// 1-based.
        line: Option<i64>,
    },
    Str(String),
    Int(i64),
}

/// One located message about a program: 1-based line and column within the
/// program text; 0 when the message has no position in it (an output the caller
/// named, a run-time stop).
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct RuleDiagnostic {
    pub line: u32,
    pub column: u32,
    pub message: String,
    /// The rule it sits in, as `name/arity`, once the parser has read its head.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
}

/// Why a program was not answered.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct RuleError {
    /// `parse` | `analysis` | `budget` | `evaluation`.
    pub kind: &'static str,
    pub diagnostics: Vec<RuleDiagnostic>,
    /// The text a surface prints: glia-rules' rendering, one diagnostic per line.
    pub rendered: String,
}

impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.rendered)
    }
}

impl std::error::Error for RuleError {}

/// One row of a rule evaluated for a caller that renders it itself (check's rule
/// invariants, DB.3a): the values with their NodeIds kept.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct RuleRow {
    pub values: Vec<Value>,
    /// The values rendered: a node's qname, a string as is, an int in decimal.
    pub subject: Vec<String>,
    /// `fact` | `derived` | `heuristic`.
    pub tier: &'static str,
    /// The first node value's location (1-based line).
    pub file: Option<String>,
    pub line: Option<i64>,
    /// The coverage caveats of the categories the row's negations read (Q5).
    pub caveats: Vec<CoverageNote>,
    /// The derivation's leaf edges, located, at most `check::MAX_EVIDENCE`.
    pub witness: Vec<ViolationEdge>,
    /// How many leaf edges the derivation has (`witness` before the cap).
    pub witness_edges: usize,
}

// ============================================================================
// The entry points
// ============================================================================

/// Answer the Datalog program `text` over `merged` (module doc).
pub fn query(
    merged: &MergedGraph,
    text: &str,
    opts: &QueryOptions,
) -> Result<QueryAnswer, RuleError> {
    let facts = CodeFacts::new(merged);
    let source = source_name(opts);
    let failed = |e: RulesError| rule_error(&e, source, opts.surface);
    let user = parse_program(source, text).map_err(failed)?;
    let program = prelude::merge(user, &opts.outputs).map_err(failed)?;
    let outputs = if opts.outputs.is_empty() {
        &program.sinks
    } else {
        &opts.outputs
    };
    let run = Run {
        outputs,
        explain: opts.explain,
        budget: opts.row_budget,
        limit: opts.limit,
    };
    let src = PreludeFacts::new(merged, &facts);
    let evaluated = run_program(&src, &program.program, &run).map_err(failed)?;
    Ok(answer(
        merged,
        &facts,
        "datalog",
        text,
        &program.program,
        evaluated,
        opts.surface,
    ))
}

/// Parse, merge the prelude into and analyse `text` against the code schema with
/// no graph: the error diagnostics, in recorded order; empty when the program is
/// valid (warnings are not errors).
pub fn validate(text: &str) -> Vec<RuleDiagnostic> {
    let empty = MergedGraph::new(Vec::new());
    let facts = CodeFacts::new(&empty);
    let src = PreludeFacts::new(&empty, &facts);
    let checked = parse_program(DEFAULT_SOURCE, text)
        .and_then(|user| prelude::merge(user, &[]))
        .and_then(|m| analyse(&m.program, &catalog_of(&src)).map(|_| ()));
    match checked {
        Ok(()) => Vec::new(),
        Err(e) => diagnostics_of(&e),
    }
}

/// Evaluate the rule program `text` (spans named `source_name`, the prelude merged
/// in) over `facts`, which must be facts over `merged`, for its one output
/// `output`, every row explained: the rows in rendered order with their NodeIds
/// kept (check renders subject and location). An error is its rendered text,
/// `<source>:<line>:<col>: error: ...` for a parse or analysis error; a program
/// that defines no `output` is ``rule defines no `<output>` predicate``. Prints no
/// `[query]` line.
pub fn evaluate_rule(
    merged: &MergedGraph,
    facts: &CodeFacts<'_>,
    source_name: &str,
    text: &str,
    output: &str,
) -> Result<Vec<RuleRow>, String> {
    let rendered = |e: RulesError| rule_error(&e, source_name, "").rendered;
    let user = parse_program(source_name, text).map_err(rendered)?;
    let defines_output = user.rules.iter().any(|r| r.head.pred == output);
    let outputs = [output.to_string()];
    let program = prelude::merge(user, &[]).map_err(rendered)?.program;
    let src = PreludeFacts::new(merged, facts);
    analyse(&program, &catalog_of(&src)).map_err(rendered)?;
    if !defines_output {
        return Err(format!("rule defines no `{output}` predicate"));
    }
    let run = Run {
        outputs: &outputs,
        explain: true,
        budget: None,
        limit: None,
    };
    let evaluated = run_program(&src, &program, &run).map_err(rendered)?;
    let mut nodes = Nodes::new(facts);
    let mut caveats = Caveats::new(merged);
    Ok(evaluated
        .outputs
        .into_iter()
        .flat_map(|o| o.rows)
        .map(|r| {
            let (file, line) = nodes.first_location(&r.values);
            RuleRow {
                subject: r.values.iter().map(|v| nodes.render(v)).collect(),
                tier: tier_name(r.tier),
                file,
                line,
                caveats: caveats.get(&r.negated),
                witness: r.witness,
                witness_edges: r.witness_edges,
                values: r.values,
            }
        })
        .collect())
}

// ============================================================================
// Evaluation
// ============================================================================

/// What [`run_program`] evaluates for.
pub(crate) struct Run<'o> {
    /// As `QueryOptions::outputs`.
    pub(crate) outputs: &'o [String],
    pub(crate) explain: bool,
    pub(crate) budget: Option<usize>,
    /// Rows kept per output after the sort (`None`: all).
    pub(crate) limit: Option<usize>,
}

/// An evaluated program: its outputs in report order (as asked, else the sinks in
/// first-definition order).
pub(crate) struct Evaluated {
    pub(crate) outputs: Vec<EvalOutput>,
}

/// One output's rows, sorted and cut.
pub(crate) struct EvalOutput {
    pub(crate) name: String,
    pub(crate) arity: usize,
    pub(crate) columns: Vec<String>,
    /// Rows before the cut.
    pub(crate) total: usize,
    pub(crate) rows: Vec<EvalRow>,
}

/// One row before rendering.
pub(crate) struct EvalRow {
    pub(crate) values: Vec<Value>,
    pub(crate) tier: Tier,
    /// The edge categories the row's negations read (registry order; every
    /// category when one reads any).
    pub(crate) negated: Vec<&'static str>,
    /// With explain: the leaf edges, located and capped.
    pub(crate) witness: Vec<ViolationEdge>,
    pub(crate) witness_edges: usize,
}

/// Evaluate `program` (the prelude already merged in, `prelude::merge`) over `src`,
/// the code facts plus the engine relations (module doc): outputs resolved and
/// ordered, rows sorted by rendered values and cut, witnesses built when asked.
/// Analysis warnings print on stderr, then the `[rules] prelude relations=` marker,
/// error or not; `src.built()` says afterwards which relations the run built.
pub(crate) fn run_program(
    src: &PreludeFacts<'_, '_>,
    program: &Program,
    run: &Run<'_>,
) -> Result<Evaluated, RulesError> {
    let outputs = evaluate_outputs(src, program, run);
    eprintln!("{}", src.marker());
    Ok(Evaluated { outputs: outputs? })
}

/// [`run_program`]'s evaluation over `src`.
fn evaluate_outputs(
    src: &PreludeFacts<'_, '_>,
    program: &Program,
    run: &Run<'_>,
) -> Result<Vec<EvalOutput>, RulesError> {
    let facts = src.facts();
    let mut options = Options::default();
    options.outputs = run.outputs.to_vec();
    if let Some(cap) = run.budget {
        options.max_rows = cap;
    }
    let mut ev = evaluate(program, src, &options)?;
    for w in ev.warnings() {
        eprintln!("{w}");
    }
    let mut negations = Negations::new(program);
    let mut sigs = ev.outputs();
    sigs.sort_by_key(|sig| output_rank(program, run.outputs, sig));
    let mut nodes = Nodes::new(facts);
    let edges: Vec<&Edge> = if run.explain {
        facts.edges().collect()
    } else {
        Vec::new()
    };
    let mut limits = ev.explain_limits();
    limits.max_leaves = MAX_EVIDENCE;
    let mut outputs = Vec::with_capacity(sigs.len());
    for sig in &sigs {
        let (name, arity) = split_signature(sig);
        let mut rows = ev.rows(sig)?;
        sort_rows(&mut rows, &mut nodes);
        let total = rows.len();
        if let Some(n) = run.limit {
            rows.truncate(n);
        }
        let witnesses = if run.explain {
            ev.explain_all(&rows, &limits)?
        } else {
            Vec::new()
        };
        let rows = rows
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let (witness, witness_edges) = match witnesses.get(i) {
                    Some(w) => locate_witness(facts, &edges, &w.leaf_edges()),
                    None => (Vec::new(), 0),
                };
                EvalRow {
                    negated: negations.categories(&r.absence),
                    tier: r.tier,
                    values: r.values,
                    witness,
                    witness_edges,
                }
            })
            .collect();
        outputs.push(EvalOutput {
            columns: columns(program, &name, arity),
            name,
            arity,
            total,
            rows,
        });
    }
    Ok(outputs)
}

/// Where output `sig` (`name/arity`) is reported: its position among the outputs
/// the caller asked for (`name` or `name/arity`), else among the predicates the
/// program defines, in first-definition order.
fn output_rank(program: &Program, asked: &[String], sig: &str) -> usize {
    let name = sig.rsplit_once('/').map_or(sig, |(n, _)| n);
    if !asked.is_empty() {
        return asked
            .iter()
            .position(|w| w == sig || w == name)
            .unwrap_or(usize::MAX);
    }
    let mut defined: Vec<String> = Vec::new();
    for r in &program.rules {
        let s = r.head.signature();
        if !defined.contains(&s) {
            defined.push(s);
        }
    }
    defined.iter().position(|d| d == sig).unwrap_or(usize::MAX)
}

/// `name/arity` -> `(name, arity)`; an unparsable arity is 0.
fn split_signature(sig: &str) -> (String, usize) {
    match sig.rsplit_once('/') {
        Some((n, a)) => (n.to_string(), a.parse().unwrap_or(0)),
        None => (sig.to_string(), 0),
    }
}

/// The column names of `name/arity`: its first rule's head, in source order.
fn columns(program: &Program, name: &str, arity: usize) -> Vec<String> {
    let Some(rule) = program
        .rules
        .iter()
        .find(|r| r.head.pred == name && r.head.arity() == arity)
    else {
        return (0..arity).map(|i| format!("c{i}")).collect();
    };
    rule.head
        .args
        .iter()
        .enumerate()
        .map(|(i, a)| match a {
            HeadArg::Term(Term::Var { name, .. }) => name.clone(),
            HeadArg::Term(_) => format!("c{i}"),
            HeadArg::Agg(g) => format!("{}({})", g.op.name(), g.vars.join(", ")),
        })
        .collect()
}

/// One value's sort key: Int < Str < Node; a node by (qname, file, line).
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Int(i64),
    Str(String),
    Node(String, Option<String>, Option<i64>),
}

/// Sort `rows` by their rendered value tuples, stably over glia-rules' order.
fn sort_rows(rows: &mut Vec<Row>, nodes: &mut Nodes<'_, '_>) {
    let keyed: Vec<Vec<Key>> = rows
        .iter()
        .map(|r| {
            r.values
                .iter()
                .map(|v| match v {
                    Value::Int(i) => Key::Int(*i),
                    Value::Str(s) => Key::Str(s.clone()),
                    Value::Node(id) => {
                        let n = nodes.get(*id);
                        Key::Node(n.qname.clone(), n.file.clone(), n.line)
                    }
                })
                .collect()
        })
        .collect();
    let mut order: Vec<usize> = (0..rows.len()).collect();
    order.sort_by(|&a, &b| keyed[a].cmp(&keyed[b]));
    let mut slots: Vec<Option<Row>> = std::mem::take(rows).into_iter().map(Some).collect();
    rows.extend(order.into_iter().filter_map(|i| slots[i].take()));
}

/// The edge categories a row's negations read (Q5), with the engine relations'
/// share memoised per negated predicate.
struct Negations<'p> {
    program: &'p Program,
    /// `name/arity` -> the categories the engine relations it reaches read.
    relations: BTreeMap<String, Vec<&'static str>>,
}

impl<'p> Negations<'p> {
    fn new(program: &'p Program) -> Self {
        Negations {
            program,
            relations: BTreeMap::new(),
        }
    }

    /// The edge categories `caveats` read, glia-rules' cones plus the engine
    /// relations each negated predicate reaches, in registry order; every category
    /// when one reads any.
    fn categories(&mut self, caveats: &[AbsenceCaveat]) -> Vec<&'static str> {
        if caveats.is_empty() {
            return Vec::new();
        }
        let any = caveats.iter().any(|c| c.any_category);
        let mut named: Vec<&str> = caveats
            .iter()
            .flat_map(|c| c.categories.iter().map(String::as_str))
            .collect();
        for c in caveats {
            let program = self.program;
            let reads = self
                .relations
                .entry(c.negated.clone())
                .or_insert_with(|| prelude::relation_reads(program, &c.negated));
            named.extend(reads.iter().copied());
        }
        registry_categories()
            .iter()
            .map(|(_, n)| *n)
            .filter(|n| any || named.contains(n))
            .collect()
    }
}

/// The code domain's edge categories, `(id, name)`.
fn registry_categories() -> &'static [(glia_core::EdgeCategoryId, &'static str)] {
    CODE_PROFILE.tables.registries.edge_categories
}

/// A witness's leaf hops as located rows: each the edge at its global ordinal
/// (`all_edges` order), its evidence site, and `why`'s tier and note; capped at
/// `MAX_EVIDENCE`. Also the hop count before the cap.
fn locate_witness(
    facts: &CodeFacts<'_>,
    edges: &[&Edge],
    hops: &[glia_rules::explain::EdgeHop],
) -> (Vec<ViolationEdge>, usize) {
    let loc = facts.locator();
    let rows = hops
        .iter()
        .take(MAX_EVIDENCE)
        .map(|h| match edges.get(h.ordinal) {
            Some(e) => {
                let ev = Evidence::of(e);
                let tn = why::tier_of(ev.as_ref(), e);
                located_edge(loc, e.from, e.category, e.to, ev.as_ref(), tn)
            }
            None => located_edge(
                loc,
                h.from,
                h.category,
                h.to,
                None,
                (tier_name(h.tier), None),
            ),
        })
        .collect();
    (rows, hops.len())
}

// ============================================================================
// Rendering
// ============================================================================

/// The located identity of one node, as a value renders it.
struct NodeAt {
    qname: String,
    kind: &'static str,
    file: Option<String>,
    line: Option<i64>,
}

/// The facts' Locator, memoised per node (looked up only, never iterated).
struct Nodes<'f, 'a> {
    facts: &'f CodeFacts<'a>,
    seen: HashMap<NodeId, NodeAt>,
}

impl<'f, 'a> Nodes<'f, 'a> {
    fn new(facts: &'f CodeFacts<'a>) -> Self {
        Nodes {
            facts,
            seen: HashMap::new(),
        }
    }

    fn get(&mut self, id: NodeId) -> &NodeAt {
        let facts = self.facts;
        self.seen.entry(id).or_insert_with(|| {
            let at = facts.locator().locate(id);
            NodeAt {
                qname: at.qname,
                kind: at.kind,
                file: at.file,
                line: at.line,
            }
        })
    }

    fn value(&mut self, v: &Value) -> QueryValue {
        match v {
            Value::Node(id) => {
                let n = self.get(*id);
                QueryValue::Node {
                    qname: n.qname.clone(),
                    kind: n.kind,
                    file: n.file.clone(),
                    line: n.line,
                }
            }
            Value::Str(s) => QueryValue::Str(s.clone()),
            Value::Int(i) => QueryValue::Int(*i),
        }
    }

    /// A node's qname, a string as is, an int in decimal.
    fn render(&mut self, v: &Value) -> String {
        match v {
            Value::Node(id) => self.get(*id).qname.clone(),
            Value::Str(s) => s.clone(),
            Value::Int(i) => i.to_string(),
        }
    }

    /// The first node value's `(file, line)`.
    fn first_location(&mut self, values: &[Value]) -> (Option<String>, Option<i64>) {
        match values.iter().find_map(|v| match v {
            Value::Node(id) => Some(*id),
            _ => None,
        }) {
            Some(id) => {
                let n = self.get(id);
                (n.file.clone(), n.line)
            }
            None => (None, None),
        }
    }
}

/// `caveats_for` per distinct category list (most rows of an output share one).
struct Caveats<'m> {
    merged: &'m MergedGraph,
    seen: BTreeMap<Vec<&'static str>, Vec<CoverageNote>>,
}

impl<'m> Caveats<'m> {
    fn new(merged: &'m MergedGraph) -> Self {
        Caveats {
            merged,
            seen: BTreeMap::new(),
        }
    }

    fn get(&mut self, cats: &[&'static str]) -> Vec<CoverageNote> {
        if cats.is_empty() {
            return Vec::new();
        }
        let merged = self.merged;
        self.seen
            .entry(cats.to_vec())
            .or_insert_with(|| caveats_for(merged, cats, None))
            .clone()
    }
}

/// The answer of an evaluated program, and its `[query]` marker.
fn answer(
    merged: &MergedGraph,
    facts: &CodeFacts<'_>,
    language: &'static str,
    text: &str,
    program: &Program,
    evaluated: Evaluated,
    surface: &'static str,
) -> QueryAnswer {
    let mut nodes = Nodes::new(facts);
    let mut caveats = Caveats::new(merged);
    let mut outputs = Vec::with_capacity(evaluated.outputs.len());
    let mut results = Vec::new();
    let mut names = Vec::with_capacity(evaluated.outputs.len());
    let mut any_row = false;
    for o in evaluated.outputs {
        names.push((o.name.clone(), o.arity));
        any_row |= o.total > 0;
        outputs.push(QueryOutput {
            predicate: o.name.clone(),
            arity: o.arity,
            columns: o.columns,
            total: o.total,
            shown: o.rows.len(),
        });
        for r in o.rows {
            let (file, line) = nodes.first_location(&r.values);
            results.push(QueryRow {
                predicate: o.name.clone(),
                values: r.values.iter().map(|v| nodes.value(v)).collect(),
                tier: tier_name(r.tier),
                file,
                line,
                caveats: caveats.get(&r.negated),
                witness: r.witness,
            });
        }
    }
    let absence = (!any_row).then(|| {
        let listed: Vec<String> = names.iter().map(|(n, a)| format!("{n}/{a}")).collect();
        let note = format!("no row of {} in this graph", listed.join(", "));
        let wanted: Vec<&str> = names.iter().map(|(n, _)| n.as_str()).collect();
        let query: String = text.chars().take(ABSENCE_QUERY_CHARS).collect();
        let mechanisms = categories_read(program, &wanted);
        absence::empty(merged, "query", &query, "no_match", note, &mechanisms, None)
    });
    let tiers = |t: &str| results.iter().filter(|r| r.tier == t).count();
    eprintln!(
        "[query] lang={language} outputs={} rows={} fact={} derived={} heuristic={} \
         caveated={} surface={surface}",
        outputs.len(),
        results.len(),
        tiers(tier_name(Tier::Fact)),
        tiers(tier_name(Tier::Derived)),
        tiers(tier_name(Tier::Heuristic)),
        results.iter().filter(|r| !r.caveats.is_empty()).count(),
    );
    QueryAnswer {
        language,
        outputs,
        results,
        absence,
    }
}

/// Every edge category the rules of `outputs` read, through the predicates they
/// reach (positive or negated, the merged prelude's included), in registry order:
/// an edge-family atom's constant category; the category set of a `reaches` /
/// `scc` call (the carry set when it names none); the categories an engine relation
/// is computed from (`prelude::RelationSpec::reads`); every category for a category
/// read through a variable, `_` or `"*"`.
fn categories_read(program: &Program, outputs: &[&str]) -> Vec<&'static str> {
    let defined = |p: &str| program.rules.iter().any(|r| r.head.pred == p);
    let mut seen: Vec<&str> = outputs.to_vec();
    let mut stack: Vec<&str> = outputs.to_vec();
    let mut named: Vec<&str> = Vec::new();
    let mut all = false;
    while let Some(p) = stack.pop() {
        for rule in program.rules.iter().filter(|r| r.head.pred == p) {
            for lit in &rule.body {
                let (Literal::Pos(atom) | Literal::Neg(atom)) = lit else {
                    continue;
                };
                let pred = atom.pred.as_str();
                if defined(pred) {
                    if !seen.contains(&pred) {
                        seen.push(pred);
                        stack.push(pred);
                    }
                    continue;
                }
                let category = atom.args.get(2);
                match (pred, atom.args.len()) {
                    ("edge", 3) | ("edge_conf", 4) | ("edge_cell", 5) => match category {
                        Some(Term::Str { value, .. }) if value != "*" => named.push(value),
                        _ => all = true,
                    },
                    ("reaches", 2) | ("scc", 2) => {
                        named.extend(CODE_PROFILE.tables.carry_edges.iter().filter_map(|c| {
                            registry_categories()
                                .iter()
                                .find(|(id, _)| id == c)
                                .map(|(_, n)| *n)
                        }));
                    }
                    ("reaches", 3) | ("reaches", 4) | ("scc", 3) => match category {
                        Some(Term::Str { value, .. }) => {
                            for part in value.split('|').map(str::trim) {
                                if part == "*" {
                                    all = true;
                                } else {
                                    named.push(part);
                                }
                            }
                        }
                        _ => all = true,
                    },
                    (name, n) => {
                        if let Some(rel) = prelude::relation(name, n) {
                            named.extend(rel.reads());
                        }
                    }
                }
            }
        }
    }
    registry_categories()
        .iter()
        .map(|(_, n)| *n)
        .filter(|n| all || named.contains(n))
        .collect()
}

// ============================================================================
// Errors
// ============================================================================

fn source_name(opts: &QueryOptions) -> &str {
    if opts.source_name.is_empty() {
        DEFAULT_SOURCE
    } else {
        &opts.source_name
    }
}

/// glia-rules' diagnostics as rule diagnostics.
fn diagnostics_of(e: &RulesError) -> Vec<RuleDiagnostic> {
    e.diagnostics().iter().map(diagnostic).collect()
}

fn diagnostic(d: &Diagnostic) -> RuleDiagnostic {
    RuleDiagnostic {
        line: d.span.line,
        column: d.span.col,
        message: d.message.clone(),
        predicate: d.predicate.clone(),
    }
}

/// How `surface` raises the row budget, for a budget stop's text.
fn budget_hint(surface: &str) -> Option<&'static str> {
    match surface {
        "cli" => Some("`--max-rows <n>`"),
        "py" => Some("`max_rows=<n>`"),
        "engine" => Some("`QueryOptions::row_budget`"),
        _ => None,
    }
}

/// `e` as a [`RuleError`] of `source`, rendered for `surface` (module doc).
fn rule_error(e: &RulesError, source: &str, surface: &str) -> RuleError {
    let unplaced = |message: String, predicate: Option<String>| RuleDiagnostic {
        line: 0,
        column: 0,
        message,
        predicate,
    };
    match e {
        RulesError::Parse(_) => RuleError {
            kind: "parse",
            diagnostics: diagnostics_of(e),
            rendered: e.to_string(),
        },
        RulesError::Invalid(_) => RuleError {
            kind: "analysis",
            diagnostics: diagnostics_of(e),
            rendered: e.to_string(),
        },
        RulesError::Unknown { .. } => RuleError {
            kind: "analysis",
            diagnostics: vec![unplaced(e.to_string(), None)],
            rendered: format!("{source}: error: {e}"),
        },
        RulesError::Budget {
            predicate, line, ..
        } => {
            let mut rendered = e.to_string();
            if let Some(hint) = budget_hint(surface) {
                rendered.push_str(&format!(" The budget is set with {hint}."));
            }
            RuleError {
                kind: "budget",
                diagnostics: vec![RuleDiagnostic {
                    line: *line,
                    column: 0,
                    message: e.to_string(),
                    predicate: Some(predicate.clone()),
                }],
                rendered,
            }
        }
        _ => RuleError {
            kind: "evaluation",
            diagnostics: vec![unplaced(e.to_string(), None)],
            rendered: format!("{source}: error: {e}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories_read_follow_the_outputs() {
        let p = parse_program(
            DEFAULT_SOURCE,
            "a(x) :- edge(x, _, \"CALLS\"), !b(x).\n\
             b(x) :- reaches(x, _, \"USES|IMPORTS\").\n\
             c(x) :- edge(x, _, k), node(x, k).",
        )
        .expect("parses");
        assert_eq!(categories_read(&p, &["a"]), ["IMPORTS", "CALLS", "USES"]);
        let all = categories_read(&p, &["c"]);
        assert_eq!(all.len(), registry_categories().len());
        assert!(categories_read(&p, &["nothing"]).is_empty());
        let carry = parse_program(DEFAULT_SOURCE, "d(y) :- node(x, \"ROUTE\"), reaches(x, y).")
            .expect("parses");
        let read = categories_read(&carry, &["d"]);
        assert_eq!(read.len(), CODE_PROFILE.tables.carry_edges.len());
    }

    #[test]
    fn budget_hint_names_each_surface() {
        assert_eq!(budget_hint("cli"), Some("`--max-rows <n>`"));
        assert_eq!(budget_hint("py"), Some("`max_rows=<n>`"));
        assert_eq!(budget_hint("engine"), Some("`QueryOptions::row_budget`"));
        assert_eq!(budget_hint(""), None);
    }
}
