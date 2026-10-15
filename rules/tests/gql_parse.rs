//! DB.7a: the GQL read subset, parsed (no graph). Every grammar row parses to the
//! expected tree (compared through the compact rendering below), every quantifier
//! spelling of one bound reads back as one `Quant`, every construct outside the
//! subset is refused by name at its keyword, and syntax errors are located.

use glia_rules::gql::ast::{
    Dir, Expr, Lit, NodePat, Operand, PathPattern, Query, RelPat, RetExpr, ReturnItem, StrOp,
};
use glia_rules::gql::{self, SOURCE_NAME};

// ============================================================================
// The compact rendering the expected trees are pinned in
// ============================================================================

/// One line per query, clauses joined by ` | `. AND / OR are parenthesised so the
/// tree's shape (precedence, associativity) is visible; an abbreviated arrow renders
/// bracketed (`-->` is `-[]->`) and every quantifier as its normalised `{min,max}`.
fn render(q: &Query) -> String {
    let mut parts = Vec::new();
    for m in &q.matches {
        let pats: Vec<String> = m.patterns.iter().map(pattern).collect();
        parts.push(format!("MATCH {}", pats.join(", ")));
    }
    if let Some(c) = &q.condition {
        parts.push(format!("WHERE {}", expr(c)));
    }
    let items: Vec<String> = q.items.iter().map(item).collect();
    parts.push(format!("RETURN {}{}", if q.distinct { "DISTINCT " } else { "" }, items.join(", ")));
    if !q.order_by.is_empty() {
        let keys: Vec<String> = q
            .order_by
            .iter()
            .map(|k| format!("{}{}", operand(&k.key), if k.descending { " DESC" } else { "" }))
            .collect();
        parts.push(format!("ORDER BY {}", keys.join(", ")));
    }
    if let Some(n) = q.limit {
        parts.push(format!("LIMIT {n}"));
    }
    parts.join(" | ")
}

fn pattern(p: &PathPattern) -> String {
    let mut s = String::new();
    if let Some(v) = &p.var {
        s.push_str(&format!("{} = ", v.name));
    }
    if p.shortest {
        s.push_str("ANY SHORTEST ");
    }
    s.push_str(&node(&p.start));
    for (r, n) in &p.steps {
        s.push_str(&rel(r));
        s.push_str(&node(n));
    }
    s
}

fn node(n: &NodePat) -> String {
    let mut s = String::from("(");
    if let Some(v) = &n.var {
        s.push_str(&v.name);
    }
    if !n.labels.is_empty() {
        let labels: Vec<&str> = n.labels.iter().map(|l| l.name.as_str()).collect();
        s.push_str(&format!(":{}", labels.join("|")));
    }
    if !n.props.is_empty() {
        let props: Vec<String> =
            n.props.iter().map(|(k, v)| format!("{}: {}", k.name, lit(v))).collect();
        s.push_str(&format!(" {{{}}}", props.join(", ")));
    }
    s.push(')');
    s
}

fn rel(r: &RelPat) -> String {
    let mut body = String::new();
    if let Some(v) = &r.var {
        body.push_str(&v.name);
    }
    if !r.types.is_empty() {
        let types: Vec<&str> = r.types.iter().map(|t| t.name.as_str()).collect();
        body.push_str(&format!(":{}", types.join("|")));
    }
    let mut s = match r.dir {
        Dir::Out => format!("-[{body}]->"),
        Dir::In => format!("<-[{body}]-"),
        Dir::Undirected => format!("-[{body}]-"),
    };
    if let Some(q) = r.quant {
        match q.max {
            Some(max) => s.push_str(&format!("{{{},{max}}}", q.min)),
            None => s.push_str(&format!("{{{},}}", q.min)),
        }
    }
    s
}

fn expr(e: &Expr) -> String {
    match e {
        Expr::Or { left, right, .. } => format!("({} OR {})", expr(left), expr(right)),
        Expr::And { left, right, .. } => format!("({} AND {})", expr(left), expr(right)),
        Expr::Not { expr: inner, .. } => format!("NOT {}", expr(inner)),
        Expr::Cmp { op, left, right, .. } => {
            format!("{} {} {}", operand(left), op.symbol(), operand(right))
        }
        Expr::StrTest { op, left, right, .. } => {
            let word = match op {
                StrOp::StartsWith => "STARTS WITH",
                StrOp::EndsWith => "ENDS WITH",
                StrOp::Contains => "CONTAINS",
            };
            format!("{} {word} {}", operand(left), operand(right))
        }
        Expr::In { left, list, .. } => {
            let list: Vec<String> = list.iter().map(lit).collect();
            format!("{} IN [{}]", operand(left), list.join(", "))
        }
        Expr::Pattern { pattern: p, exists: true, .. } => format!("EXISTS {{{}}}", pattern(p)),
        Expr::Pattern { pattern: p, .. } => pattern(p),
        Expr::Call { name, args, .. } => {
            let args: Vec<String> = args.iter().map(operand).collect();
            format!("{}({})", name.name, args.join(", "))
        }
    }
}

fn operand(o: &Operand) -> String {
    match o {
        Operand::Var(v) => v.name.clone(),
        Operand::Prop { var, key, .. } => format!("{}.{}", var.name, key.name),
        Operand::Lit(l) => lit(l),
        Operand::Type { var, .. } => format!("type({})", var.name),
    }
}

fn lit(l: &Lit) -> String {
    match l {
        Lit::Str { value, .. } => format!("{value:?}"),
        Lit::Int { value, .. } => value.to_string(),
        Lit::Bool { value: true, .. } => "TRUE".to_string(),
        Lit::Bool { value: false, .. } => "FALSE".to_string(),
    }
}

fn item(i: &ReturnItem) -> String {
    let mut s = match &i.expr {
        RetExpr::Operand(o) => operand(o),
        RetExpr::Agg(a) => {
            let arg = match &a.arg {
                None => "*".to_string(),
                Some(o) if a.distinct => format!("DISTINCT {}", operand(o)),
                Some(o) => operand(o),
            };
            format!("{}({arg})", a.op.name())
        }
    };
    if let Some(alias) = &i.alias {
        s.push_str(&format!(" AS {}", alias.name));
    }
    s
}

// ============================================================================
// Helpers
// ============================================================================

fn ok(text: &str) -> Query {
    match gql::parse(text) {
        Ok(q) => q,
        Err(diags) => {
            let shown: Vec<String> = diags.iter().map(|d| d.to_string()).collect();
            panic!("parse failed on {text:?}:\n{}", shown.join("\n"))
        }
    }
}

fn errors(text: &str) -> Vec<String> {
    match gql::parse(text) {
        Ok(q) => panic!("parsed {text:?} without error: {}", render(&q)),
        Err(diags) => diags.iter().map(|d| d.to_string()).collect(),
    }
}

/// The source text a span covers (spans are byte ranges on char boundaries).
fn slice(text: &str, span: glia_rules::diag::Span) -> &str {
    text.get(span.start as usize..span.end as usize).unwrap_or("<not a char boundary>")
}

/// The bounds of the first relationship's quantifier in `MATCH (a)<rel>(b) RETURN b`.
fn quant(rel: &str) -> Option<(u32, Option<u32>)> {
    let q = ok(&format!("MATCH (a){rel}(b) RETURN b"));
    q.matches[0].patterns[0].steps[0].0.quant.map(|q| q.bounds())
}

// ============================================================================
// (1) Every grammar row parses to its expected tree
// ============================================================================

#[test]
fn every_construct_parses() {
    let table: [(&str, &str); 30] = [
        // Nodes: bare, one label, label alternatives, a property map of each literal kind.
        ("MATCH (a) RETURN a", "MATCH (a) | RETURN a"),
        ("MATCH (a:ROUTE) RETURN a", "MATCH (a:ROUTE) | RETURN a"),
        ("MATCH (f:FUNCTION|METHOD) RETURN f.qname", "MATCH (f:FUNCTION|METHOD) | RETURN f.qname"),
        (
            "MATCH (a:ROUTE {qname: 'web::get', line: 3, live: true, depth: -1}) RETURN a",
            "MATCH (a:ROUTE {qname: \"web::get\", line: 3, live: TRUE, depth: -1}) | RETURN a",
        ),
        // The three directions, with and without a variable, and the abbreviated arrows.
        (
            "MATCH (:ROUTE)-[:HANDLED_BY]->(h) RETURN h",
            "MATCH (:ROUTE)-[:HANDLED_BY]->(h) | RETURN h",
        ),
        ("MATCH (a)<-[r:CALLS]-(b) RETURN a, b, r", "MATCH (a)<-[r:CALLS]-(b) | RETURN a, b, r"),
        ("MATCH (a)-[:IMPORTS]-(b) RETURN b", "MATCH (a)-[:IMPORTS]-(b) | RETURN b"),
        ("MATCH (a)-->(b)<--(c)--(d) RETURN d", "MATCH (a)-[]->(b)<-[]-(c)-[]-(d) | RETURN d"),
        // Every quantifier spelling: Cypher inside the brackets, GQL after them.
        (
            "MATCH (a:ROUTE)-[:CALLS|USES*1..3]->(b) RETURN b",
            "MATCH (a:ROUTE)-[:CALLS|USES]->{1,3}(b) | RETURN b",
        ),
        ("MATCH (a)-[*]->(b) RETURN b", "MATCH (a)-[]->{1,}(b) | RETURN b"),
        ("MATCH (a)-[r:CALLS*2]->(b) RETURN b", "MATCH (a)-[r:CALLS]->{2,2}(b) | RETURN b"),
        ("MATCH (a)-[:CALLS*2..]->(b) RETURN b", "MATCH (a)-[:CALLS]->{2,}(b) | RETURN b"),
        ("MATCH (a)-[:CALLS*..4]-(b) RETURN b", "MATCH (a)-[:CALLS]-{1,4}(b) | RETURN b"),
        ("MATCH (a)-[:CALLS]->+(b) RETURN b", "MATCH (a)-[:CALLS]->{1,}(b) | RETURN b"),
        ("MATCH (a)-[:CALLS]->*(b) RETURN b", "MATCH (a)-[:CALLS]->{0,}(b) | RETURN b"),
        (
            "MATCH (a)-[:CALLS]->{2,5}(b)-[:USES]->{3}(c) RETURN c",
            "MATCH (a)-[:CALLS]->{2,5}(b)-[:USES]->{3,3}(c) | RETURN c",
        ),
        ("MATCH (a)<-[:CALLS]-{1,}(b) RETURN b", "MATCH (a)<-[:CALLS]-{1,}(b) | RETURN b"),
        // A path variable under ANY SHORTEST; two MATCH clauses, one with two patterns.
        (
            "MATCH p = ANY SHORTEST (a:ROUTE)-[:CALLS]->+(b:DATA) RETURN p",
            "MATCH p = ANY SHORTEST (a:ROUTE)-[:CALLS]->{1,}(b:DATA) | RETURN p",
        ),
        (
            "MATCH (a), (b:MODULE) MATCH (a)-[:DEFINES]->(b) RETURN a, b",
            "MATCH (a), (b:MODULE) | MATCH (a)-[:DEFINES]->(b) | RETURN a, b",
        ),
        // WHERE: precedence (NOT > AND > OR), left association, every comparison.
        (
            "MATCH (a) WHERE a.qname = 'x' AND a.line <> 3 OR a.line != 4 RETURN a",
            "MATCH (a) | WHERE ((a.qname = \"x\" AND a.line != 3) OR a.line != 4) | RETURN a",
        ),
        (
            "MATCH (a) WHERE NOT a.x < 1 AND (a.y <= 2 OR a.z > 3) AND a.w >= 4 RETURN a",
            "MATCH (a) | WHERE ((NOT a.x < 1 AND (a.y <= 2 OR a.z > 3)) AND a.w >= 4) | RETURN a",
        ),
        (
            "MATCH (a) WHERE a.qname STARTS WITH 'web::' OR a.qname ENDS WITH \"::main\" \
             OR a.qname CONTAINS 'it\\'s' RETURN a",
            "MATCH (a) | WHERE ((a.qname STARTS WITH \"web::\" OR a.qname ENDS WITH \"::main\") \
             OR a.qname CONTAINS \"it's\") | RETURN a",
        ),
        (
            "MATCH (a) WHERE a.kind IN ['ROUTE', 'ENDPOINT', 3, -2, false] RETURN a",
            "MATCH (a) | WHERE a.kind IN [\"ROUTE\", \"ENDPOINT\", 3, -2, FALSE] | RETURN a",
        ),
        // Pattern predicates: NOT pattern and EXISTS { pattern }.
        (
            "MATCH (r:ROUTE) WHERE NOT (r)-[:HANDLED_BY]->() RETURN r",
            "MATCH (r:ROUTE) | WHERE NOT (r)-[:HANDLED_BY]->() | RETURN r",
        ),
        (
            "MATCH (r:ROUTE) WHERE EXISTS { (r)-[:HANDLED_BY]->(:FUNCTION) } RETURN r",
            "MATCH (r:ROUTE) | WHERE EXISTS {(r)-[:HANDLED_BY]->(:FUNCTION)} | RETURN r",
        ),
        // A function call (a domain builtin) and a doubled NOT.
        (
            "MATCH (a) WHERE in_scope(a, 'web') AND NOT NOT a.live = TRUE RETURN a",
            "MATCH (a) | WHERE (in_scope(a, \"web\") AND NOT NOT a.live = TRUE) | RETURN a",
        ),
        // type(r) in WHERE and RETURN.
        (
            "MATCH (a)-[r]->(b) WHERE type(r) = 'CALLS' OR type(r) IN ['USES'] RETURN type(r), b.qname",
            "MATCH (a)-[r]->(b) | WHERE (type(r) = \"CALLS\" OR type(r) IN [\"USES\"]) \
             | RETURN type(r), b.qname",
        ),
        // Aggregates and aliases.
        (
            "MATCH (a)-[:CALLS]->(b) RETURN a.qname AS f, count(*), count(DISTINCT b) AS n, \
             min(b.line), max(b.line), sum(b.size)",
            "MATCH (a)-[:CALLS]->(b) | RETURN a.qname AS f, count(*), count(DISTINCT b) AS n, \
             min(b.line), max(b.line), sum(b.size)",
        ),
        // RETURN DISTINCT, ORDER BY with ASC / DESC / default, LIMIT, a trailing `;`.
        (
            "MATCH (a)-[:CALLS]->(b) RETURN DISTINCT a.qname AS f, count(b) AS n \
             ORDER BY n DESC, f ASC, a.qname LIMIT 10;",
            "MATCH (a)-[:CALLS]->(b) | RETURN DISTINCT a.qname AS f, count(b) AS n \
             | ORDER BY n DESC, f, a.qname | LIMIT 10",
        ),
        // Lines and comments; a pattern first in WHERE; literal return items; LIMIT 0.
        (
            "MATCH (a)\n  WHERE (a)-[:CALLS]->(a) AND a.n >= -5 // self-call\nRETURN a, 'x', 7 LIMIT 0",
            "MATCH (a) | WHERE ((a)-[:CALLS]->(a) AND a.n >= -5) | RETURN a, \"x\", 7 | LIMIT 0",
        ),
    ];
    for (text, want) in table {
        assert_eq!(render(&ok(text)), want, "for {text:?}");
    }
}

#[test]
fn spans_slice_back_to_their_text() {
    let text = "MATCH (a:ROUTE)-[:CALLS*1..3]->(b)-[:USES]->{2,4}(c)\nWHERE a.qname STARTS WITH 'é' \
                RETURN a.qname, count(b) AS n";
    let q = ok(text);
    assert_eq!(slice(text, q.span), text);
    let p = &q.matches[0].patterns[0];
    assert_eq!(
        slice(text, q.matches[0].span),
        "MATCH (a:ROUTE)-[:CALLS*1..3]->(b)-[:USES]->{2,4}(c)"
    );
    assert_eq!(slice(text, p.start.span), "(a:ROUTE)");
    assert_eq!(slice(text, p.start.labels[0].span), "ROUTE");
    let (r1, b) = &p.steps[0];
    assert_eq!(slice(text, r1.span), "-[:CALLS*1..3]->");
    assert_eq!(r1.quant.map(|q| slice(text, q.span)), Some("*1..3"));
    assert_eq!(slice(text, b.span), "(b)");
    let r2 = &p.steps[1].0;
    assert_eq!(slice(text, r2.span), "-[:USES]->{2,4}");
    assert_eq!(r2.quant.map(|q| slice(text, q.span)), Some("{2,4}"));
    let cond = q.condition.as_ref().map(|c| slice(text, c.span()));
    assert_eq!(cond, Some("a.qname STARTS WITH 'é'"));
    let cond_span = q.condition.as_ref().map(|c| (c.span().line, c.span().col));
    assert_eq!(cond_span, Some((2, 7)));
    let items: Vec<&str> = q.items.iter().map(|i| slice(text, i.span)).collect();
    assert_eq!(items, vec!["a.qname", "count(b) AS n"]);
    match &q.items[1].expr {
        RetExpr::Agg(a) => assert_eq!(slice(text, a.span), "count(b)"),
        other => panic!("expected an aggregate, got {other:?}"),
    }
}

// ============================================================================
// (2) Every quantifier spelling of one bound agrees
// ============================================================================

#[test]
fn quantifier_spellings_agree() {
    for rel in
        ["-[:CALLS*1..]->", "-[:CALLS*]->", "-[:CALLS]->+", "-[:CALLS]->{1,}", "-[:CALLS*..]->"]
    {
        assert_eq!(quant(rel), Some((1, None)), "for {rel}");
    }
    assert_eq!(quant("-[:CALLS*2..4]->"), quant("-[:CALLS]->{2,4}"));
    assert_eq!(quant("-[:CALLS*2..4]->"), Some((2, Some(4))));
    for rel in ["-[:CALLS*0..]->", "-[:CALLS]->*", "-[:CALLS]->{0,}"] {
        assert_eq!(quant(rel), Some((0, None)), "for {rel}");
    }
    for rel in ["-[:CALLS*3]->", "-[:CALLS*3..3]->", "-[:CALLS]->{3}", "-[:CALLS]->{3,3}"] {
        assert_eq!(quant(rel), Some((3, Some(3))), "for {rel}");
    }
    assert_eq!(quant("-[:CALLS*..2]->"), quant("-[:CALLS]->{1,2}"));
    assert_eq!(quant("-[:CALLS]->"), None);
    // An untyped relationship may take zero hops exactly (the endpoints coincide).
    assert_eq!(quant("-[*0]->"), Some((0, Some(0))));
}

// ============================================================================
// (3) Everything outside the subset is refused by name, once, at its keyword
// ============================================================================

#[test]
fn outside_the_subset_is_refused() {
    let pinned = [
        (
            "MATCH (a)\nOPTIONAL MATCH (a)-->(b)\nRETURN a, b",
            "<gql>:2:1: error: `OPTIONAL MATCH` is not in the GQL read subset (docs/rules.md)",
        ),
        (
            "MATCH (a) CREATE (a)-[:T]->(b) RETURN a",
            "<gql>:1:11: error: `CREATE` is not in the GQL read subset (docs/rules.md)",
        ),
        (
            "MATCH (a) WITH a RETURN a",
            "<gql>:1:11: error: `WITH` is not in the GQL read subset (docs/rules.md)",
        ),
        (
            "MATCH (a) UNWIND [1, 2] AS x RETURN a",
            "<gql>:1:11: error: `UNWIND` is not in the GQL read subset (docs/rules.md)",
        ),
        (
            "MATCH p = shortestPath((a)-[:T*]->(b)) RETURN p",
            "<gql>:1:11: error: `shortestPath()` is not in the GQL read subset (docs/rules.md): \
             write `p = ANY SHORTEST (a)-[:T]->+(b)`",
        ),
        (
            "MATCH p = TRAIL (a)-[:CALLS]->+(b) RETURN p",
            "<gql>:1:11: error: `TRAIL` is not in the GQL read subset (docs/rules.md): glia \
             returns endpoint pairs under walk semantics (Q4)",
        ),
        (
            "MATCH (a) WHERE a.qname = $p RETURN a",
            "<gql>:1:27: error: parameter `$p` is not in the GQL read subset (docs/rules.md): \
             write the value as a literal",
        ),
    ];
    for (text, want) in pinned {
        assert_eq!(errors(text), vec![want.to_string()], "for {text:?}");
    }

    // The rest of the refused list: one diagnostic each, naming the construct.
    let named = [
        ("MATCH (a) RETURN a UNION MATCH (b) RETURN b", "UNION", (1, 20)),
        ("MATCH (a) CALL db.labels() RETURN a", "CALL", (1, 11)),
        ("MERGE (a:X) RETURN a", "MERGE", (1, 1)),
        ("MATCH (a) SET a.x = 1 RETURN a", "SET", (1, 11)),
        ("MATCH (a) DELETE a", "DELETE", (1, 11)),
        ("MATCH (a) DETACH DELETE a", "DETACH DELETE", (1, 11)),
        ("MATCH (a) REMOVE a.x RETURN a", "REMOVE", (1, 11)),
        ("MATCH (a) FOREACH (x IN [1] | SET a.x = x) RETURN a", "FOREACH", (1, 11)),
        ("LOAD CSV FROM 'f' AS row MATCH (a) RETURN a", "LOAD", (1, 1)),
        ("MATCH (a) RETURN a SKIP 1 LIMIT 2", "SKIP", (1, 20)),
        ("MATCH (a) RETURN CASE WHEN a.x = 1 THEN 1 END", "CASE", (1, 18)),
        ("MATCH p = ALL SHORTEST (a)-[:T]->+(b) RETURN p", "ALL SHORTEST", (1, 11)),
        ("MATCH p = allShortestPaths((a)-[:T*]->(b)) RETURN p", "allShortestPaths()", (1, 11)),
        ("MATCH (a) WHERE shortestPath((a)-[*]->(a)) RETURN a", "shortestPath()", (1, 17)),
        ("MATCH ACYCLIC (a)-[:T]->+(b) RETURN b", "ACYCLIC", (1, 7)),
        ("MATCH p = ANY SHORTEST SIMPLE (a)-[:T]->+(b) RETURN p", "SIMPLE", (1, 24)),
        ("match (a) optional match (b) return a", "OPTIONAL MATCH", (1, 11)),
        ("MATCH (a {qname: $q}) RETURN a", "$q", (1, 18)),
    ];
    for (text, name, (line, col)) in named {
        let got = errors(text);
        assert_eq!(got.len(), 1, "for {text:?}: {got:?}");
        let prefix = format!("<gql>:{line}:{col}: error: ");
        assert!(got[0].starts_with(&prefix), "for {text:?}: {got:?}");
        assert!(got[0].contains(&format!("`{name}`")), "for {text:?}: {got:?}");
        assert!(got[0].contains("is not in the GQL read subset (docs/rules.md)"), "{got:?}");
        let restrictor = ["TRAIL", "ACYCLIC", "SIMPLE"].contains(&name);
        assert_eq!(got[0].contains("walk semantics (Q4)"), restrictor, "for {text:?}: {got:?}");
    }
}

// ============================================================================
// (4) Syntax errors are located; one query reports every error
// ============================================================================

#[test]
fn syntax_errors_are_located() {
    let cases: [(&str, &[&str]); 14] = [
        // A lexical error comes back as the shared lexer's diagnostic.
        ("MATCH (a) WHERE a.q = 'abc RETURN a", &["<gql>:1:23: error: unterminated string"]),
        (
            "MATCH (a)-[:T*3..1]->(b) RETURN b",
            &["<gql>:1:14: error: the quantifier's lower bound 3 exceeds its upper bound 1"],
        ),
        (
            "MATCH (a)-[:T]->{4,2}(b) RETURN b",
            &["<gql>:1:17: error: the quantifier's lower bound 4 exceeds its upper bound 2"],
        ),
        (
            "MATCH (a)-[:T|U*0]->(b) RETURN b",
            &["<gql>:1:16: error: a quantifier of at most 0 hops never takes a `T|U` \
               relationship: drop the type or raise the upper bound"],
        ),
        (
            "MATCH (a)-[:T*1..2]->+(b) RETURN b",
            &["<gql>:1:22: error: a relationship takes one quantifier, `*m..n` inside the \
               brackets or `+` / `*` / `{m,n}` after them, not both"],
        ),
        (
            "MATCH (a:ROUTE RETURN a",
            &["<gql>:1:16: error: expected `)` to close the node pattern, found `RETURN`"],
        ),
        // Two errors in two clauses: two diagnostics.
        (
            "MATCH (a:ROUTE RETURN a.",
            &[
                "<gql>:1:16: error: expected `)` to close the node pattern, found `RETURN`",
                "<gql>:1:25: error: expected a property key after `.`, found end of input",
            ],
        ),
        (
            "MATCH (a)-[:T*3..1]->(b)\nWHERE a.x =\nRETURN b",
            &[
                "<gql>:1:14: error: the quantifier's lower bound 3 exceeds its upper bound 1",
                "<gql>:3:1: error: expected a variable, `var.key`, a literal or type(r), found \
                 `RETURN`",
            ],
        ),
        // The greedy-token caveats get a hint each.
        (
            "MATCH (a {n:-1}) RETURN a",
            &["<gql>:1:12: error: `:-` reads as one token here: write `n: -1`"],
        ),
        (
            "MATCH (a) WHERE a.n<-1 RETURN a",
            &["<gql>:1:20: error: `<-` reads as an arrow here: write `a.n < -1`"],
        ),
        // Clause structure.
        ("", &["<gql>:1:1: error: expected MATCH, found end of input"]),
        (
            "MATCH (a)",
            &["<gql>:1:10: error: expected `,`, MATCH, WHERE or RETURN, found end of input"],
        ),
        (
            "MATCH (a) RETURN a WHERE a.x = 1",
            &["<gql>:1:20: error: `WHERE` is out of place: a query reads MATCH .. [WHERE ..] \
               RETURN [DISTINCT] .. [ORDER BY ..] [LIMIT n]"],
        ),
        (
            "MATCH (a) RETURN a LIMIT -1",
            &["<gql>:1:26: error: expected a non-negative integer after LIMIT, found `-`"],
        ),
    ];
    for (text, want) in cases {
        assert_eq!(errors(text), want.to_vec(), "for {text:?}");
    }

    // Aggregates and functions where the subset does not take them.
    let where_agg = errors("MATCH (a) WHERE count(a) > 1 RETURN a");
    assert_eq!(where_agg, vec!["<gql>:1:17: error: aggregate `count()` is allowed only in RETURN"]);
    let star = errors("MATCH (a) RETURN min(*)");
    assert_eq!(star, vec!["<gql>:1:22: error: `*` is allowed only in count(*)"]);
    let order = errors("MATCH (a) RETURN count(a) ORDER BY count(a)");
    assert_eq!(
        order,
        vec![
            "<gql>:1:36: error: ORDER BY takes a column alias or `var.key`: alias the \
              expression (`count(b) AS n`) and order by the alias"
        ]
    );
    let func = errors("MATCH (a) RETURN toUpper(a.qname)");
    assert_eq!(
        func,
        vec![
            "<gql>:1:18: error: function `toUpper()` is not in the GQL read subset \
              (docs/rules.md): a value is a variable, `var.key`, a literal or type(r)"
        ]
    );
    // A call in WHERE is a condition (a boolean builtin), never a compared value.
    let compared = errors("MATCH (a) WHERE toUpper(a.qname) = 'X' RETURN a");
    assert_eq!(
        compared,
        vec![
            "<gql>:1:17: error: function `toUpper()` is not in the GQL read subset \
              (docs/rules.md) as a value: a condition may call a boolean builtin \
              (`in_scope(a, 'web')`), and a value is a variable, `var.key`, a literal or type(r)"
        ]
    );
    let any = errors("MATCH (a) WHERE a.x = 1 MATCH (b) RETURN b");
    assert_eq!(
        any,
        vec![
            "<gql>:1:25: error: `MATCH` is out of place: a query reads MATCH .. [WHERE ..] \
              RETURN [DISTINCT] .. [ORDER BY ..] [LIMIT n]"
        ]
    );
    let shortest = errors("MATCH ANY SHORTEST (a)-[:T]->+(b) RETURN b");
    assert_eq!(
        shortest,
        vec![
            "<gql>:1:7: error: `ANY SHORTEST` needs a path variable: `p = ANY SHORTEST \
              (a)-[:T]->+(b)`"
        ]
    );
}

#[test]
fn diagnostics_cap_at_20() {
    let text = format!("MATCH (a) RETURN a{}", " WHERE a.x = 1".repeat(30));
    let got = errors(&text);
    assert_eq!(got.len(), 20);
    assert!(got.iter().all(|d| d.contains("`WHERE` is out of place")), "{got:?}");
    // A condition nested past the depth limit is one located error, not a stack overflow.
    let deep = format!("MATCH (a) WHERE {}a.x = 1{} RETURN a", "(".repeat(500), ")".repeat(500));
    let got = errors(&deep);
    assert_eq!(got, vec!["<gql>:1:81: error: the condition nests deeper than 64 levels"]);
}

// ============================================================================
// (5) Keywords are case-insensitive; identifiers are not
// ============================================================================

#[test]
fn keywords_are_case_insensitive() {
    assert_eq!(render(&ok("match (a) return a")), "MATCH (a) | RETURN a");
    let lower =
        ok("MaTcH p = any shortest (a:Route)-[:calls]->+(b) wHeRe a.x starts with 'x' and not \
         b.y in [TRUE] Return Distinct a.x As X, CoUnT(DiStInCt b) Order By X desc Limit 1");
    assert_eq!(
        render(&lower),
        "MATCH p = ANY SHORTEST (a:Route)-[:calls]->{1,}(b) | WHERE (a.x STARTS WITH \"x\" AND \
         NOT b.y IN [TRUE]) | RETURN DISTINCT a.x AS X, count(DISTINCT b) | ORDER BY X DESC \
         | LIMIT 1"
    );
    // A keyword spelling as a label, relationship type or property key stays an
    // identifier; as a variable or alias it is reserved.
    assert_eq!(
        render(&ok("MATCH (a:Order {limit: 1})-[r:WHERE]->(b:match) RETURN a.return, r.order")),
        "MATCH (a:Order {limit: 1})-[r:WHERE]->(b:match) | RETURN a.return, r.order"
    );
    assert_eq!(
        errors("MATCH (a) RETURN a AS order"),
        vec!["<gql>:1:23: error: expected an alias after AS, found `order`"]
    );
    assert_eq!(SOURCE_NAME, "<gql>");
}
