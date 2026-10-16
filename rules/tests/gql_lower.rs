//! DB.7b: the GQL read subset lowered to the rule IR, checked by EVALUATION over an
//! inline FactSource (the DA toy pattern, Q10): 7 nodes of kinds MODULE / FUNCTION,
//! categories CALLS / IMPORTS / DEFINES, the attribute `qname`, the call chain
//! f1 -> f2 -> f3 -> f4 plus f1 -> f4. A few lowered programs are pinned as text so
//! the shape (helpers, DNF, range restriction, singletons as `_`) is reviewable.

use glia_activation::profile::{DomainTables, Registries};
use glia_core::{Confidence, EdgeCategoryId, NodeId, NodeKindId, RepoId};
use glia_rules::eval::{Options, evaluate};
use glia_rules::gql::{self, Compiled, OrderKey};
use glia_rules::ir::{AttrDecl, ColType};
use glia_rules::source::MemSource;
use glia_rules::testkit;
use glia_rules::value::{Tier, Value};

// ============================================================================
// The fixture
// ============================================================================

const FUNCTION: NodeKindId = NodeKindId(1);
const MODULE: NodeKindId = NodeKindId(2);
const CALLS: EdgeCategoryId = EdgeCategoryId(1);
const IMPORTS: EdgeCategoryId = EdgeCategoryId(2);
const DEFINES: EdgeCategoryId = EdgeCategoryId(3);

static KINDS: [(NodeKindId, &str); 2] = [(FUNCTION, "FUNCTION"), (MODULE, "MODULE")];
static CATEGORIES: [(EdgeCategoryId, &str); 3] =
    [(CALLS, "CALLS"), (IMPORTS, "IMPORTS"), (DEFINES, "DEFINES")];
static CARRY: [EdgeCategoryId; 1] = [CALLS];

fn tables() -> DomainTables {
    DomainTables {
        graph_type: "gql-lower-test",
        registries: Registries {
            node_kinds: &KINDS,
            edge_categories: &CATEGORIES,
            cell_types: &[],
        },
        carry_edges: &CARRY,
        ..testkit::tables()
    }
}

fn id(name: &str) -> NodeId {
    let kind = if name.starts_with('f') { FUNCTION } else { MODULE };
    NodeId::from_parts("gql-lower-test", RepoId(0), kind, name)
}

/// f1..f4 (FUNCTION) and m1..m3 (MODULE); every node but m3 has a `qname`.
/// CALLS f1->f2, f2->f3, f3->f4, f1->f4; DEFINES m1->f1, m1->f2, m2->f3, m2->f4;
/// IMPORTS m1->m2, m2->m3.
fn fixture() -> MemSource {
    let mut g = MemSource::new(tables());
    g.attr(AttrDecl { name: "qname", ty: ColType::Str });
    for n in ["f1", "f2", "f3", "f4", "m1", "m2", "m3"] {
        let kind = if n.starts_with('f') { FUNCTION } else { MODULE };
        g.node(id(n), kind, n);
        if n != "m3" {
            g.set_attr(id(n), 0, Value::Str(n.to_string()));
        }
    }
    let edges = [
        ("f1", "f2", CALLS),
        ("f2", "f3", CALLS),
        ("f3", "f4", CALLS),
        ("f1", "f4", CALLS),
        ("m1", "f1", DEFINES),
        ("m1", "f2", DEFINES),
        ("m2", "f3", DEFINES),
        ("m2", "f4", DEFINES),
        ("m1", "m2", IMPORTS),
        ("m2", "m3", IMPORTS),
    ];
    for (a, b, c) in edges {
        g.edge(id(a), id(b), c, Confidence::Strong, Tier::Fact);
    }
    g
}

fn compile(text: &str) -> Compiled {
    match gql::compile(text) {
        Ok(c) => c,
        Err(d) => panic!("{text}: {d:?}"),
    }
}

/// The `result` rows of `text`, each value rendered, sorted as the evaluator sorts.
fn rows(text: &str) -> Vec<Vec<String>> {
    let c = compile(text);
    let g = fixture();
    let mut opts = Options::default();
    opts.outputs = vec![c.output.clone()];
    let ev = match evaluate(&c.program, &g, &opts) {
        Ok(ev) => ev,
        Err(e) => panic!("{text}\n{}\n{e}", c.program),
    };
    assert!(ev.warnings().is_empty(), "{text}: {:?}\n{}", ev.warnings(), c.program);
    let rows = ev.rows(&c.output).expect("result rows");
    rows.iter().map(|r| r.values.iter().map(|v| ev.render(v)).collect()).collect()
}

/// The rendered rows, one column each.
fn col(text: &str) -> Vec<String> {
    rows(text).into_iter().map(|r| r.join(",")).collect()
}

/// The diagnostics' messages of a query that must fail.
fn errors(text: &str) -> Vec<String> {
    match gql::compile(text) {
        Ok(c) => panic!("{text} compiled:\n{}", c.program),
        Err(d) => d.iter().map(|d| d.message.clone()).collect(),
    }
}

fn r(cols: &[&str]) -> Vec<String> {
    cols.iter().map(|c| c.to_string()).collect()
}

// ============================================================================
// The acceptance cases
// ============================================================================

#[test]
fn typed_hop() {
    let text = "MATCH (a:FUNCTION)-[:CALLS]->(b) RETURN a.qname, b.qname";
    let c = compile(text);
    assert_eq!(c.output, "result");
    assert_eq!(c.columns, ["a.qname", "b.qname"]);
    assert_eq!(c.order_by, []);
    assert_eq!(c.limit, None);
    assert_eq!(c.path_column, None);
    assert_eq!(c.program.sources, ["<gql>"]);
    assert_eq!(
        c.program.to_string(),
        "result(_g0, _g1) :- node(a, \"FUNCTION\"), edge(a, b, \"CALLS\"), qname(a, _g0), \
         qname(b, _g1).\n"
    );
    assert_eq!(
        rows(text),
        [r(&["f1", "f2"]), r(&["f1", "f4"]), r(&["f2", "f3"]), r(&["f3", "f4"])]
    );
}

#[test]
fn reaches_and_bounds() {
    let star = "MATCH (a {qname: 'f1'})-[:CALLS*]->(b) RETURN b.qname";
    assert_eq!(col(star), ["f2", "f3", "f4"]);
    assert!(compile(star).program.to_string().contains("reaches(a, b, \"CALLS\")"));
    // Walk length exactly 2: f4 is reached at 1 (f1 -> f4) and 3, never at 2.
    assert_eq!(col("MATCH (a {qname: 'f1'})-[:CALLS*2..2]->(b) RETURN b.qname"), ["f3"]);
    assert_eq!(
        col("MATCH (a {qname: 'f1'})-[:CALLS*0..]->(b) RETURN b.qname"),
        ["f1", "f2", "f3", "f4"]
    );
    // Every spelling of one bound lowers alike: {2,} is two-or-more.
    assert_eq!(col("MATCH (a {qname: 'f1'})-[:CALLS]->{2,}(b) RETURN b.qname"), ["f3", "f4"]);
    assert_eq!(
        col("MATCH (a {qname: 'f1'})-[:CALLS*1..3]->(b) RETURN b.qname"),
        ["f2", "f3", "f4"]
    );
    // <- reads right to left; an untyped quantifier walks every category.
    assert_eq!(col("MATCH (b {qname: 'f4'})<-[:CALLS*]-(a) RETURN a.qname"), ["f1", "f2", "f3"]);
    assert_eq!(
        col("MATCH (m {qname: 'm1'})-[*]->(x) RETURN x"),
        ["f1", "f2", "f3", "f4", "m2", "m3"]
    );
    assert_eq!(
        errors("MATCH (a)-[:CALLS*1..9]->(b) RETURN a"),
        ["bounded repetition above 8 hops: use * (any length)"]
    );
    // Bounded undirected repetition unrolls through a two-way hop helper.
    let undirected = "MATCH (a {qname: 'f3'})-[:CALLS*2..2]-(b) RETURN b.qname";
    assert_eq!(col(undirected), ["f1", "f3"]);
    assert!(
        compile(undirected)
            .program
            .to_string()
            .contains("gql_hop0(x, y) :- edge(y, x, \"CALLS\").")
    );
}

#[test]
fn not_pattern_is_a_negated_helper() {
    let text = "MATCH (f:FUNCTION) WHERE NOT ()-[:CALLS]->(f) RETURN f.qname";
    assert_eq!(col(text), ["f1"]);
    let c = compile(text);
    let not0 = c.program.rules.iter().filter(|r| r.head.pred == "gql_not0").count();
    assert_eq!(not0, 1);
    assert_eq!(
        c.program.to_string(),
        "result(_g1) :- node(f, \"FUNCTION\"), !gql_not0(f), qname(f, _g1).\n\
         gql_not0(f) :- edge(_, f, \"CALLS\").\n"
    );
    // A negated EXISTS reads the same way; a positive one is inlined.
    assert_eq!(
        col("MATCH (f:FUNCTION) WHERE NOT EXISTS { (f)-[:CALLS]->() } RETURN f.qname"),
        ["f4"]
    );
    assert_eq!(
        col("MATCH (f:FUNCTION) WHERE EXISTS { (f)-[:CALLS]->() } RETURN f.qname"),
        ["f1", "f2", "f3"]
    );
    // A pattern sharing no variable negates an arity-0 helper: some IMPORTS edge exists.
    let closed = "MATCH (f:FUNCTION) WHERE NOT ()-[:IMPORTS]->() RETURN f";
    assert!(compile(closed).program.to_string().contains("gql_not0 :- edge(_, _, \"IMPORTS\")."));
    assert_eq!(col(closed), Vec::<String>::new());
    assert_eq!(
        col("MATCH (f:FUNCTION) WHERE NOT ()-[:IMPORTS]->(f) RETURN f"),
        ["f1", "f2", "f3", "f4"]
    );
}

/// Names resolve in analysis, located in the GQL text: the lowering keeps the spans.
#[test]
fn analysis_errors_point_into_the_query() {
    let c = compile("MATCH (a:ROUTEE) RETURN a");
    let err = evaluate(&c.program, &fixture(), &Options::default()).err().expect("unknown kind");
    let text = err.to_string();
    assert!(text.starts_with("<gql>:1:10: error: unknown node kind `ROUTEE`"), "{text}");
}

#[test]
fn dnf_and_in() {
    let text = "MATCH (a:FUNCTION) WHERE a.qname = 'f1' OR a.qname IN ['f2', 'f3'] RETURN a.qname";
    assert_eq!(col(text), ["f1", "f2", "f3"]);
    let c = compile(text);
    let result = c.program.rules.iter().filter(|r| r.head.pred == "result").count();
    assert_eq!(result, 2, "{}", c.program);
    let facts: Vec<String> = c
        .program
        .rules
        .iter()
        .filter(|r| r.head.pred == "gql_in0")
        .map(|r| r.to_string())
        .collect();
    assert_eq!(facts, ["gql_in0(\"f2\").", "gql_in0(\"f3\")."]);
    // NOT pushes inward: NOT (a OR b) is (NOT a) AND (NOT b); NOT IN is the negated table.
    assert_eq!(
        col(
            "MATCH (a:FUNCTION) WHERE NOT (a.qname = 'f1' OR a.qname IN ['f2', 'f3']) RETURN a.qname"
        ),
        ["f4"]
    );
    let or3 = "(a.qname = 'f1' OR a.qname = 'f2' OR a.qname = 'f3')";
    let big =
        format!("MATCH (a:FUNCTION) WHERE {or3} AND {or3} AND {or3} AND {or3} RETURN a.qname");
    assert_eq!(errors(&big), ["the query expands to 81 alternatives (limit 64): split it"]);
}

#[test]
fn aggregates_group() {
    let text =
        "MATCH (a:FUNCTION)-[:CALLS]->(b) RETURN a.qname AS f, count(b) AS n ORDER BY n DESC";
    let c = compile(text);
    assert_eq!(c.columns, ["f", "n"]);
    assert_eq!(c.order_by, [OrderKey { column: 1, descending: true }]);
    assert_eq!(
        c.program.to_string(),
        "result(_g0, count(b)) :- node(a, \"FUNCTION\"), edge(a, b, \"CALLS\"), qname(a, _g0).\n"
    );
    assert_eq!(rows(text), [r(&["f1", "2"]), r(&["f2", "1"]), r(&["f3", "1"])]);
    // count(*) counts distinct bindings of every MATCH variable; DISTINCT changes nothing.
    assert_eq!(col("MATCH (a)-[:CALLS]->(b) RETURN count(*)"), ["4"]);
    assert_eq!(col("MATCH (a)-[:CALLS]->(b) RETURN count(DISTINCT a)"), ["3"]);
    // ORDER BY and LIMIT are kept for the engine (it sorts rendered values, DB.7c), so
    // the program still answers every group.
    let text =
        "MATCH (m:MODULE)-[:DEFINES]->(f) RETURN m.qname, count(f) AS n ORDER BY m.qname LIMIT 1";
    assert_eq!(col(text), ["m1,2", "m2,2"]);
    let c = compile(text);
    assert_eq!((c.order_by, c.limit), (vec![OrderKey { column: 0, descending: false }], Some(1)));
    // An aggregate over several alternatives reads one gql_match union.
    let union = "MATCH (a:FUNCTION|MODULE)-[:CALLS|DEFINES]->(b) RETURN a.qname, count(b)";
    assert_eq!(col(union), ["f1,2", "f2,1", "f3,1", "m1,2", "m2,2"]);
    let program = compile(union).program.to_string();
    assert!(program.contains("result(_g0, count(b)) :- gql_match0(_g0, b)."), "{program}");
    assert_eq!(compile(union).limit, None);
    let limited = compile("MATCH (a) RETURN a ORDER BY a LIMIT 3");
    assert_eq!(
        (limited.order_by, limited.limit),
        (vec![OrderKey { column: 0, descending: false }], Some(3))
    );
}

#[test]
fn homomorphism() {
    let got = rows("MATCH (a)-[:CALLS]->(b), (c)-[:CALLS]->(b) RETURN a.qname, c.qname");
    assert!(got.contains(&r(&["f1", "f1"])), "{got:?}");
    assert_eq!(
        got,
        [r(&["f1", "f1"]), r(&["f1", "f3"]), r(&["f2", "f2"]), r(&["f3", "f1"]), r(&["f3", "f3"])]
    );
}

/// No nulls: `m.qname` on a node without a `qname` drops the row (m3), where the bare
/// variable keeps it.
#[test]
fn missing_attribute_drops_the_row() {
    assert_eq!(col("MATCH (m:MODULE) RETURN m.qname"), ["m1", "m2"]);
    assert_eq!(col("MATCH (m:MODULE) RETURN m"), ["m1", "m2", "m3"]);
    // ... also under NOT: a missing attribute never satisfies a negated test.
    assert_eq!(col("MATCH (m:MODULE) WHERE NOT m.qname STARTS WITH 'x' RETURN m"), ["m1", "m2"]);
    assert_eq!(col("MATCH (m:MODULE) WHERE NOT m.qname = 'm1' RETURN m"), ["m2"]);
}

#[test]
fn same_text_same_program() {
    let texts = [
        "MATCH (a:FUNCTION)-[:CALLS]->(b) RETURN a.qname, b.qname",
        "MATCH (a:FUNCTION|MODULE)-[r:CALLS|DEFINES]-(b)-[:CALLS*0..2]->(c) WHERE NOT (a.qname STARTS WITH 'f' OR (b)-[:IMPORTS]->()) AND type(r) <> 'X' RETURN a, type(r), count(*) ORDER BY a",
        "MATCH p = (a)-[:CALLS*]->(b) WHERE a.qname IN ['f1', 'f2'] RETURN p, b.qname",
    ];
    for text in texts {
        let (one, two) = (compile(text), compile(text));
        assert_eq!(one, two, "{text}");
        assert_eq!(one.program.to_string(), two.program.to_string());
    }
}

// ============================================================================
// The rest of the mapping
// ============================================================================

#[test]
fn relationship_types_and_directions() {
    // type(r) is the category variable; several types read one hop helper.
    assert_eq!(
        col("MATCH (a {qname: 'm1'})-[r]->(b) RETURN type(r), b.qname"),
        ["DEFINES,f1", "DEFINES,f2", "IMPORTS,m2"]
    );
    assert_eq!(
        col("MATCH (a {qname: 'm1'})-[r:IMPORTS|DEFINES]->(b) WHERE type(r) = 'IMPORTS' RETURN b"),
        ["m2"]
    );
    assert_eq!(
        col("MATCH (a {qname: 'f2'})-[r:CALLS]-(b) RETURN b, type(r)"),
        ["f1,CALLS", "f3,CALLS"]
    );
    assert_eq!(col("MATCH (a)<-[:DEFINES]-(m {qname: 'm2'}) RETURN a"), ["f3", "f4"]);
    // Two labels are two alternatives.
    assert_eq!(col("MATCH (a:FUNCTION|MODULE)-[:IMPORTS]->(b) RETURN a, b"), ["m1,m2", "m2,m3"]);
}

#[test]
fn conditions_and_builtins() {
    assert_eq!(col("MATCH (a:FUNCTION) WHERE a.qname > 'f2' RETURN a"), ["f3", "f4"]);
    assert_eq!(col("MATCH (a:FUNCTION) WHERE NOT a.qname > 'f2' RETURN a"), ["f1", "f2"]);
    assert_eq!(col("MATCH (a) WHERE a.qname ENDS WITH '1' RETURN a"), ["f1", "m1"]);
    assert_eq!(
        col("MATCH (a) WHERE a.qname CONTAINS 'm' AND NOT a.qname CONTAINS '2' RETURN a"),
        ["m1"]
    );
    // A condition helper's head is the variables it shares (the node and its attribute).
    let program =
        compile("MATCH (a) WHERE NOT a.qname STARTS WITH 'f' RETURN a").program.to_string();
    assert!(
        program.contains("gql_not0(a, _g0) :- qname(a, _g0), starts_with(_g0, \"f\")."),
        "{program}"
    );
    // A function call is the builtin of that name: `in_scope` is unknown to this source,
    // so analysis (later) names it at the call; the lowering just emits it.
    let call = compile("MATCH (a) WHERE in_scope(a, 'web') RETURN a");
    assert!(call.program.to_string().contains("in_scope(a, \"web\")"), "{}", call.program);
}

#[test]
fn path_column() {
    let c = compile("MATCH p = (a {qname: 'f1'})-[:CALLS*]->(b) RETURN b.qname, p ORDER BY p");
    assert_eq!(c.columns, ["b.qname", "p"]);
    assert_eq!(c.path_column, Some(1));
    assert_eq!(c.order_by, [OrderKey { column: 1, descending: false }]);
    // The path column holds the path's start; one trailing column, its end, keeps
    // two paths with different endpoints apart.
    let head = &c.program.rules[0].head;
    assert_eq!(head.to_string(), "result(_g0, a, b)");
    assert_eq!(
        rows("MATCH p = ANY SHORTEST (a {qname: 'f1'})-[:CALLS*]->(b) RETURN p"),
        [r(&["f1", "f2"]), r(&["f1", "f3"]), r(&["f1", "f4"])]
    );
    assert_eq!(
        errors("MATCH p = (a)-[:CALLS]->(b) RETURN p"),
        [
            "path variable `p` needs exactly one variable-length relationship in its pattern (found 0)"
        ]
    );
    assert_eq!(
        errors("MATCH p = (a)-[:CALLS*]->(b)-[:CALLS*]->(c) RETURN p"),
        [
            "path variable `p` needs exactly one variable-length relationship in its pattern (found 2)"
        ]
    );
}

#[test]
fn diagnostics_are_located() {
    let d = gql::compile("MATCH (a) RETURN a ORDER BY b").expect_err("unknown key");
    assert_eq!(
        d[0].to_string(),
        "<gql>:1:29: error: ORDER BY `b` names no RETURN column: order by a returned item or its alias"
    );
    assert_eq!(errors("MATCH (a) RETURN b"), ["`b` is not bound by MATCH"]);
    assert_eq!(
        errors("MATCH (a)-[r]->(b) RETURN r"),
        ["`r` is a relationship variable: use type(r)"]
    );
    assert_eq!(
        errors("MATCH (a)-[r:CALLS*]->(b) RETURN type(r)"),
        ["`r` names a variable-length relationship, which has no single type"]
    );
    assert_eq!(
        errors("MATCH (a)-[r]->(b), (c)-[r]->(d) RETURN a"),
        [
            "`r` is already a relationship variable: a relationship or path variable names one pattern"
        ]
    );
    assert_eq!(
        errors("MATCH (a) WHERE a.gql_not0 = 'x' RETURN a"),
        [
            "`gql_not0` is reserved for the lowering (`result` and `gql_*` helpers): rename the property"
        ]
    );
    assert_eq!(
        errors("MATCH (a {flag: TRUE}) RETURN a"),
        [
            "boolean literals are not rule values (the rule IR holds nodes, strings and integers): compare the attribute's own value"
        ]
    );
    assert_eq!(
        errors("MATCH (a)-[:CALLS*]-(b) RETURN a"),
        [
            "a variable-length relationship needs a direction (`->` or `<-`): reachability follows edges one way"
        ]
    );
    // A parse error comes back unchanged.
    let d = gql::compile("OPTIONAL MATCH (a) RETURN a").expect_err("refused");
    assert!(d[0].message.contains("OPTIONAL"), "{d:?}");
}
