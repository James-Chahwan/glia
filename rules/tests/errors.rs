//! DA.16: every diagnostic class of glia-rules pinned by its exact text, end to end
//! through `eval::run` (lex, parse, analyse, evaluate) as DB's `check` / `query` call
//! it, plus the run-time errors only the evaluator raises and the warnings an `Ok`
//! carries.
//!
//! The wording is an interface: DB prints these texts verbatim (check's exit-2 errors
//! rows, the `glia gaps` rot report), so a change to a message in lexer.rs, parser.rs,
//! analysis.rs (`mod msg`), aggregate.rs, source.rs or diag.rs moves this table with
//! it. Every expected string is a literal. Columns count chars, not bytes: the
//! `column_counts_chars` row puts a two-byte char before its error.
//!
//! The graph is testkit's `lcg_graph` (kinds FUNCTION / MODULE / ROUTE / OBJECT,
//! categories CALLS / IMPORTS / USES / HANDLED_BY / TESTS / NEXT, no cell types) with
//! one domain builtin, `in_scope(Node, Str)` (the Str column Must): scope `cli` holds
//! nodes 0 and 1, and any other scope is an error.

use glia_core::NodeId;
use glia_rules::diag::{RulesError, Severity};
use glia_rules::eval::{Options, run};
use glia_rules::ir::{BindReq, BuiltinDecl, BuiltinKind, ColType};
use glia_rules::source::{Builtin, BuiltinRow, MemSource};
use glia_rules::testkit::{self, GraphSpec};
use glia_rules::value::{Tier, Value, ValueRef};

/// `<name>(Node, Str)`: the nodes of scope `cli`; any other scope fails the call.
struct InScope {
    name: &'static str,
    cli: Vec<NodeId>,
}

impl Builtin for InScope {
    fn decl(&self) -> BuiltinDecl {
        BuiltinDecl {
            name: self.name.into(),
            cols: vec![ColType::Node, ColType::Str],
            bind: vec![BindReq::May, BindReq::Must],
            tiered: false,
            kind: BuiltinKind::Domain,
        }
    }

    fn call(&self, args: &[Option<ValueRef<'_>>], out: &mut Vec<BuiltinRow>) -> Result<(), String> {
        let [node, Some(ValueRef::Str(scope))] = args else {
            return Err("in_scope takes a node and a bound scope".into());
        };
        if *scope != "cli" {
            return Err(format!("scope `{scope}` holds no node"));
        }
        for id in &self.cli {
            if node.is_none_or(|n| n == ValueRef::Node(*id)) {
                let values = vec![Value::Node(*id), Value::Str((*scope).into())];
                out.push(BuiltinRow { values, tier: Tier::Fact });
            }
        }
        Ok(())
    }
}

/// The test graph with `in_scope/2`; `conflict` also registers a domain builtin
/// `edge/2`, which the catalog refuses with a warning.
fn graph(conflict: bool) -> MemSource {
    let spec = GraphSpec {
        nodes: 12,
        edges: 30,
        categories: vec!["CALLS", "USES"],
        tier_mix: [60, 30, 10],
        seed: 16,
        salt: 0,
    };
    let mut g = testkit::lcg_graph(&spec);
    let cli = vec![testkit::node_id(0, 0), testkit::node_id(0, 1)];
    g.builtin(Box::new(InScope { name: "in_scope", cli }));
    if conflict {
        g.builtin(Box::new(InScope { name: "edge", cli: Vec::new() }));
    }
    g
}

/// What one program must produce.
enum Want {
    /// `run` fails: the error's variant and its Display.
    Err(&'static str, &'static str),
    /// `run` succeeds and `rows(output)` fails with this Display.
    RowsErr(&'static str, &'static str),
    /// `run` succeeds with exactly these warnings, in order.
    Warns(&'static [&'static str]),
}

struct Case {
    name: &'static str,
    text: String,
    opts: Options,
    conflict: bool,
    want: Want,
}

fn case(name: &'static str, text: &str, want: Want) -> Case {
    Case { name, text: text.to_string(), opts: Options::default(), conflict: false, want }
}

fn variant(e: &RulesError) -> &'static str {
    match e {
        RulesError::Parse(_) => "Parse",
        RulesError::Invalid(_) => "Invalid",
        RulesError::Budget { .. } => "Budget",
        RulesError::Builtin { .. } => "Builtin",
        RulesError::Type { .. } => "Type",
        RulesError::Unknown { .. } => "Unknown",
        _ => "another variant",
    }
}

/// `wide(0, 0, ..)` with `n` arguments.
fn wide_fact(n: usize) -> String {
    format!("wide({}).", vec!["0"; n].join(", "))
}

const UNSEEDED_TC: &str = "rules:1:1: warning: recursive predicate `tc/2` is unseeded: its \
    recursion starts from every row of `edge/3`; seed it (`tc(x, y) :- seed(x), ...`) or use \
    reaches(x, y, \"CATS\"); magic sets seed it when every caller binds an argument (in the \
    rule for `tc/2`)";

const TC: &str = "tc(x, y) :- edge(x, y, \"CALLS\").\ntc(x, z) :- tc(x, y), edge(y, z, \"CALLS\").";

fn cases() -> Vec<Case> {
    use Want::{Err, RowsErr, Warns};
    let mut budget = case("row_budget", TC, Err("Budget", ""));
    budget.opts.max_rows = 20;
    budget.want = Err(
        "Budget",
        "rule evaluation stopped: `tc/2` (rules:1) reached 22 derived rows against a row budget \
         of 20. Its recursion is probably unseeded: bind an argument from a small relation \
         (`tc(x, y) :- seed(x), ...`), use reaches(x, y, \"CATS\") for reachability, or raise the \
         budget.",
    );
    let two = "a(x) :- node(x, \"ROUTE\").\nb(x, y) :- edge(x, y, \"CALLS\").";
    let mut outputs = case("unknown_output_option", two, Err("Unknown", ""));
    outputs.opts.outputs = vec!["nope".into()];
    outputs.want = Err("Unknown", "unknown output `nope`; the program defines a/1, b/2");
    let mut conflict = case(
        "catalog_conflict_warning",
        "v(x) :- node(x, k).",
        Warns(&[
            "rules:0:0: warning: domain builtin `edge/2` is ignored: `edge` is already the \
             predicate `edge/3`",
            "rules:1:17: warning: variable `k` appears once; write `_` (in the rule for `v/1`)",
        ]),
    );
    conflict.conflict = true;
    vec![
        // (1) lexical: no clause head is known, so no predicate suffix.
        case(
            "unterminated_string",
            "v(x) :- node(x,\"FUNCTION).",
            Err("Parse", "rules:1:16: error: unterminated string"),
        ),
        case(
            "unexpected_character",
            "v(x) :- node(x, _) @ .",
            Err("Parse", "rules:1:20: error: unexpected character `@`"),
        ),
        case(
            "unknown_escape",
            "ok(\"a\\q\").",
            Err(
                "Parse",
                "rules:1:4: error: unknown escape `\\q` in string (escapes: \\\\ \\\" \\' \\n \\t)",
            ),
        ),
        case(
            "unterminated_comment",
            "v(x) :- node(x, _). /* never closed",
            Err("Parse", "rules:1:21: error: unterminated block comment"),
        ),
        case(
            "integer_out_of_range",
            "n(99999999999999999999).",
            Err("Parse", "rules:1:3: error: integer out of range"),
        ),
        // (2) parse: a body error names the rule; a head error cannot.
        case(
            "missing_dot",
            "v(x) :- node(x, \"FUNCTION\")\nw(x) :- node(x, \"ROUTE\").",
            Err(
                "Parse",
                "rules:2:1: error: expected `.` after the rule for `v/1`, found `w` (in the rule \
                 for `v/1`)",
            ),
        ),
        case(
            "head_term",
            "v(x, ) :- node(x, _).",
            Err("Parse", "rules:1:6: error: expected a term, found `)`"),
        ),
        case(
            "unknown_aggregate",
            "v(x, avg(y)) :- edge(x, y, \"CALLS\").",
            Err("Parse", "rules:1:6: error: unknown aggregate `avg`"),
        ),
        case(
            "wildcard_fact",
            "allowed(\"web\", _).",
            Err(
                "Parse",
                "rules:1:16: error: a fact may not contain `_` (in the rule for `allowed/2`)",
            ),
        ),
        // (3) / (4) unknown names, with the nearest known name when one is close.
        case(
            "unknown_predicate",
            "v(x) :- edeg(x, y, \"CALLS\").",
            Err(
                "Invalid",
                "rules:1:9: error: unknown predicate `edeg/3`; did you mean `edge`? (in the rule \
                 for `v/1`)",
            ),
        ),
        case(
            "unknown_predicate_idb",
            "routes(x) :- node(x, \"ROUTE\").\nv(x) :- route(x).",
            Err(
                "Invalid",
                "rules:2:9: error: unknown predicate `route/1`; did you mean `routes`? (in the \
                 rule for `v/1`)",
            ),
        ),
        case(
            "unknown_predicate_no_suggestion",
            "v(x) :- node(x, _), handled(x).",
            Err(
                "Invalid",
                "rules:1:21: error: unknown predicate `handled/1` (in the rule for `v/1`)",
            ),
        ),
        case(
            "column_counts_chars",
            "/* réseau */ v(x) :- edeg(x, y, \"CALLS\").",
            Err(
                "Invalid",
                "rules:1:22: error: unknown predicate `edeg/3`; did you mean `edge`? (in the rule \
                 for `v/1`)",
            ),
        ),
        case(
            "unknown_kind",
            "v(x) :- node(x, \"ROTUE\").",
            Err(
                "Invalid",
                "rules:1:17: error: unknown node kind `ROTUE`; did you mean `ROUTE`? (in the rule \
                 for `v/1`)",
            ),
        ),
        case(
            "unknown_category",
            "v(x) :- edge(x, _, \"CALS\").",
            Err(
                "Invalid",
                "rules:1:20: error: unknown edge category `CALS`; did you mean `CALLS`? (in the \
                 rule for `v/1`)",
            ),
        ),
        case(
            "unknown_category_in_set",
            "v(x, y) :- reaches(x, y, \"CALLS|NOPE\").",
            Err(
                "Invalid",
                "rules:1:26: error: unknown edge category `NOPE` in the category set \
                 `CALLS|NOPE` (in the rule for `v/2`)",
            ),
        ),
        case(
            "unknown_category_in_set_suggests",
            "v(x, y) :- reaches(x, y, \"USE|CALLS\").",
            Err(
                "Invalid",
                "rules:1:26: error: unknown edge category `USE` in the category set `USE|CALLS`; \
                 did you mean `USES`? (in the rule for `v/2`)",
            ),
        ),
        case(
            "unknown_cell_type",
            "v(x) :- cell(x, \"CODE\", _).",
            Err("Invalid", "rules:1:17: error: unknown cell type `CODE` (in the rule for `v/1`)"),
        ),
        case(
            "constant_argument",
            "v(x, y) :- reaches(x, y, z).\nw(x, y) :- reaches(x, y, \"CALLS\", \"deep\").\nu(x) :- \
             node(x, 3).",
            Err(
                "Invalid",
                "rules:1:26: error: argument 3 of `reaches/3` must be a quoted category set (in \
                 the rule for `v/2`)\nrules:2:35: error: argument 4 of `reaches/4` must be an \
                 integer literal (in the rule for `w/2`)\nrules:3:17: error: argument 2 of \
                 `node/2` must be a quoted node kind (in the rule for `u/1`)",
            ),
        ),
        // (5) arity, naming both.
        case(
            "arity_edb",
            "v(x) :- edge(x, _).",
            Err(
                "Invalid",
                "rules:1:9: error: predicate `edge` is used with 2 arguments here but defined \
                 with 3 (in the rule for `v/1`)",
            ),
        ),
        case(
            "arity_builtin",
            "v(x) :- node(x, _), reaches(x).",
            Err(
                "Invalid",
                "rules:1:21: error: predicate `reaches` is used with 1 argument here but defined \
                 with 2, 3 or 4 (in the rule for `v/1`)",
            ),
        ),
        case(
            "arity_idb",
            "p(x) :- node(x, _).\np(x, y) :- edge(x, y, \"CALLS\").",
            Err(
                "Invalid",
                "rules:2:1: error: predicate `p` is used with 2 arguments here but defined with 1 \
                 (in the rule for `p/2`)",
            ),
        ),
        Case {
            text: wide_fact(257),
            ..case(
                "arity_cap",
                "",
                Err(
                    "Invalid",
                    "rules:1:1: error: a predicate may have at most 256 arguments; `wide` has 257 \
                     (in the rule for `wide/257`)",
                ),
            )
        },
        // (6) safety.
        case(
            "unsafe_head",
            "v(x, y) :- node(x, _).",
            Err(
                "Invalid",
                "rules:1:6: error: variable `y` in the head of `v/2` is not bound by the body (in \
                 the rule for `v/2`)",
            ),
        ),
        case(
            "unbound_negated",
            "v(x) :- node(x, _), !edge(x, y, \"CALLS\").",
            Err(
                "Invalid",
                "rules:1:30: error: variable `y` in the negated atom `edge/3` is not bound by a \
                 positive atom (in the rule for `v/1`)",
            ),
        ),
        case(
            "unbound_comparison",
            "v(x) :- node(x, _), x != y.",
            Err(
                "Invalid",
                "rules:1:26: error: variable `y` in a comparison is not bound by a positive atom \
                 (in the rule for `v/1`)",
            ),
        ),
        case(
            "wildcard_head",
            "v(_) :- node(_, \"ROUTE\").",
            Err(
                "Invalid",
                "rules:1:3: error: `_` may not appear in a head (in the rule for `v/1`)",
            ),
        ),
        case(
            "wildcard_comparison",
            "v(x) :- node(x, _), x != _.",
            Err(
                "Invalid",
                "rules:1:26: error: `_` may not appear in a comparison (in the rule for `v/1`)",
            ),
        ),
        case(
            "unbound_aggregated",
            "c(x, count(y)) :- node(x, _).",
            Err(
                "Invalid",
                "rules:1:6: error: aggregated variable `y` of `c/2` is not bound by the body (in \
                 the rule for `c/2`)",
            ),
        ),
        // (7) stratification.
        case(
            "negation_cycle",
            "p :- !q().\nq :- !p().",
            Err(
                "Invalid",
                "rules:1:7: error: negation inside recursion: `p/0`, `q/0` depend on each other \
                 through this negated atom (in the rule for `p/0`)",
            ),
        ),
        case(
            "aggregate_cycle",
            "n(x, count(y)) :- m(x, y).\nm(x, y) :- n(x, y).",
            Err(
                "Invalid",
                "rules:1:6: error: aggregation inside recursion: `m/2`, `n/2` depend on each \
                 other through this aggregate (in the rule for `n/2`)",
            ),
        ),
        case(
            "aggregate_self_cycle",
            "c(x, count(y)) :- edge(x, y, \"CALLS\"), c(y, _).",
            Err(
                "Invalid",
                "rules:1:6: error: aggregation inside recursion: `c/2` depends on itself through \
                 this aggregate (in the rule for `c/2`)",
            ),
        ),
        // (8) aggregates.
        case(
            "aggregate_in_body",
            "v(x) :- count(x).",
            Err(
                "Parse",
                "rules:1:9: error: an aggregate (count) may appear only in a head (in the rule for \
                 `v/1`)",
            ),
        ),
        case(
            "aggregate_rules",
            "f(y, count(x)) :- edge(x, y, \"CALLS\").\nf(y, 0) :- node(y, \"ROUTE\").",
            Err(
                "Invalid",
                "rules:1:1: error: `f/2` has an aggregate head, so it must have exactly one rule \
                 (found 2) (in the rule for `f/2`)",
            ),
        ),
        case(
            "aggregate_variables",
            "s(x, sum(a, b)) :- edge(x, a, \"CALLS\"), edge(x, b, \"USES\").\nm(x, min(a, b), \
             max(a, b)) :- edge(x, a, \"CALLS\"), edge(x, b, \"USES\").",
            Err(
                "Invalid",
                "rules:1:6: error: `sum` takes one variable, found 2 (in the rule for `s/2`)\n\
                 rules:2:6: error: `min` takes one variable, found 2 (in the rule for `m/3`)\n\
                 rules:2:17: error: `max` takes one variable, found 2 (in the rule for `m/3`)",
            ),
        ),
        // (9) builtin binding.
        case(
            "builtin_needs_bound",
            "v(x) :- node(x, _), in_scope(x, p).",
            Err(
                "Invalid",
                "rules:1:21: error: `in_scope/2` needs argument 2 bound before the call (in the \
                 rule for `v/1`)",
            ),
        ),
        case(
            "string_builtin_needs_bound",
            "v(x) :- node(x, _), starts_with(s, \"n\").",
            Err(
                "Invalid",
                "rules:1:21: error: `starts_with/2` needs argument 1 bound before the call (in \
                 the rule for `v/1`)",
            ),
        ),
        case(
            "negated_builtin_wildcard",
            "v(x) :- node(x, _), !starts_with(_, \"n\").",
            Err(
                "Invalid",
                "rules:1:22: error: `starts_with/2` needs argument 1 bound before the call (in \
                 the rule for `v/1`)",
            ),
        ),
        // (10) names a rule may not define.
        case(
            "define_edge",
            "edge(x, y, \"CALLS\") :- node(x, _), node(y, _).",
            Err(
                "Invalid",
                "rules:1:1: error: `edge/3` is a built-in predicate and cannot be defined by a \
                 rule (in the rule for `edge/3`)",
            ),
        ),
        case(
            "define_reaches",
            "reaches(x, y) :- edge(x, y, \"CALLS\").",
            Err(
                "Invalid",
                "rules:1:1: error: `reaches/2` is a built-in predicate and cannot be defined by a \
                 rule (in the rule for `reaches/2`)",
            ),
        ),
        case(
            "reserved_name",
            "__m(x) :- node(x, _).",
            Err(
                "Invalid",
                "rules:1:1: error: predicate names starting `__` are reserved (in the rule for \
                 `__m/1`)",
            ),
        ),
        // Several diagnostics render one per line, in source order.
        case(
            "several",
            "v(x) :- edeg(x).\nw(y, z) :- node(y, \"ROTUE\").",
            Err(
                "Invalid",
                "rules:1:9: error: unknown predicate `edeg/1`; did you mean `edge`? (in the rule \
                 for `v/1`)\nrules:2:6: error: variable `z` in the head of `w/2` is not bound \
                 by the body (in the rule for `w/2`)\nrules:2:20: error: unknown node kind \
                 `ROTUE`; did you mean `ROUTE`? (in the rule for `w/2`)",
            ),
        ),
        // (11) run time.
        case(
            "sum_over_str",
            "w(x, sum(k)) :- node(x, k).",
            Err("Type", "sum over a non-integer value in `w/2`"),
        ),
        case(
            "builtin_failed",
            "v(x) :- node(x, _), in_scope(x, \"nowhere\").",
            Err("Builtin", "builtin `in_scope/2` failed: scope `nowhere` holds no node"),
        ),
        case(
            "rows_unknown",
            two,
            RowsErr("nope", "unknown output `nope`; the program defines a/1, b/2"),
        ),
        outputs,
        budget,
        // (12) warnings ride an Ok: the singletons in rule order, then the unseeded.
        case(
            "warnings",
            &format!("{TC}\nv(x) :- node(x, k)."),
            Warns(&[
                "rules:3:17: warning: variable `k` appears once; write `_` (in the rule for `v/1`)",
                UNSEEDED_TC,
            ]),
        ),
        conflict,
    ]
}

/// What `c` produced, in `Want`'s terms, or why it is not that shape.
fn outcome(c: &Case) -> Result<String, String> {
    let src = graph(c.conflict);
    let res = run("rules", &c.text, &src, &c.opts);
    match (&c.want, res) {
        (Want::Err(..), Err(e)) => {
            let d = e.diagnostics();
            if d.iter().any(|d| d.severity != Severity::Error) {
                return Err(format!("a non-error diagnostic in {e:?}"));
            }
            let lines = e.to_string().lines().count();
            if !d.is_empty() && lines != d.len() {
                return Err(format!("{} diagnostics render as {lines} lines", d.len()));
            }
            Ok(format!("{}: {e}", variant(&e)))
        }
        (Want::RowsErr(output, _), Ok(ev)) => match ev.rows(output) {
            Err(e) => Ok(e.to_string()),
            Ok(rows) => Err(format!("rows({output:?}) answered {} rows", rows.len())),
        },
        (Want::Warns(_), Ok(ev)) => {
            if ev.warnings().iter().any(|w| w.severity != Severity::Warning) {
                return Err(format!("a non-warning in {:?}", ev.warnings()));
            }
            Ok(ev.warnings().iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"))
        }
        (_, Err(e)) => Err(format!("run failed: {e}")),
        (_, Ok(ev)) => Err(format!("run succeeded with outputs {:?}", ev.outputs())),
    }
}

#[test]
fn error_texts() {
    let mut failures = Vec::new();
    for c in cases() {
        let want = match &c.want {
            Want::Err(class, text) => format!("{class}: {text}"),
            Want::RowsErr(_, text) => (*text).to_string(),
            Want::Warns(w) => w.join("\n"),
        };
        match outcome(&c) {
            Ok(got) if got == want => {}
            Ok(got) => failures.push(format!("{}:\n  want {want}\n  got  {got}", c.name)),
            Err(why) => failures.push(format!("{}:\n  want {want}\n  but  {why}", c.name)),
        }
    }
    let n = failures.len();
    assert!(failures.is_empty(), "{n} of the table differ:\n{}", failures.join("\n"));
}

/// The arity cap is 256 (relation and plan columns are u8): 256 arguments evaluate.
#[test]
fn arity_256_evaluates() {
    let src = graph(false);
    let ev = match run("rules", &wide_fact(256), &src, &Options::default()) {
        Ok(ev) => ev,
        Err(e) => panic!("a 256-argument fact must evaluate: {e}"),
    };
    let rows = ev.rows("wide").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].values, vec![Value::Int(0); 256]);
    assert!(ev.warnings().is_empty(), "{:?}", ev.warnings());
}
