//! DA.19: the row budget gate (plan section 2 DA; R1 section 1.3). The budget must
//! stop an unseeded recursion and one exploding round before it allocates, with an
//! actionable message, while the seeded forms of the same question (a seed relation,
//! `reaches`, the magic rewrite) stay far under it. The real-scale blow-up (7,252,713
//! rows at glia x10) is DA.17's cost gate; here the mechanism is pinned on graphs a
//! debug build runs in seconds.
//!
//! Each stop prints the evaluator's `[rules] budget pred=<name>/<arity> rows=<n>
//! cap=<cap>` (or explain's `[rules] explain budget ...`) on stderr; `--nocapture`
//! shows it.

use glia_core::{Confidence, NodeId};
use glia_rules::diag::RulesError;
use glia_rules::eval::{DEFAULT_MAX_ROWS, Evaluation, Options, run};
use glia_rules::explain::ExplainLimits;
use glia_rules::source::MemSource;
use glia_rules::testkit::{GraphSpec, category_id, clique_ring, lcg_graph, node_id};
use glia_rules::value::{Tier, Value};

/// The budget every case runs under.
const CAP: usize = 50_000;

/// An unseeded transitive closure, its first rule on line 2.
const UNSEEDED_TC: &str = r#"// an unseeded transitive closure
tc(x, y) :- edge(x, y, "CALLS").
tc(x, z) :- tc(x, y), edge(y, z, "CALLS").
"#;

/// The 400-node ring with 1,200 chords: strongly connected, so the unseeded closure is
/// 160,000 rows.
fn ring() -> MemSource {
    clique_ring(400, 1_200, "CALLS", 7)
}

fn capped(max_rows: usize) -> Options {
    let mut opts = Options::default();
    opts.max_rows = max_rows;
    opts
}

/// The budget error of an evaluation that must stop.
fn stopped(text: &str, g: &MemSource, opts: &Options) -> RulesError {
    match run("rules", text, g, opts) {
        Ok(ev) => panic!("evaluation passed a budget of {}: {:?}", opts.max_rows, ev.stats()),
        Err(e) => e,
    }
}

fn evaluated<'s>(text: &str, g: &'s MemSource, opts: &Options) -> Evaluation<'s, MemSource> {
    run("rules", text, g, opts).unwrap_or_else(|e| panic!("evaluation failed: {e}"))
}

/// The renders of `pred`'s rows, one string per row.
fn rendered(ev: &Evaluation<'_, MemSource>, pred: &str) -> Vec<String> {
    let rows = ev.rows(pred).unwrap_or_else(|e| panic!("rows({pred}): {e}"));
    rows.iter()
        .map(|r| r.values.iter().map(|v| ev.render(v)).collect::<Vec<_>>().join(" "))
        .collect()
}

/// Every node of the ring, rendered, in node order.
fn every_node() -> Vec<String> {
    (0..400).map(|i| format!("n{i:05}")).collect()
}

#[test]
fn unseeded_tc_hits_the_budget() {
    let g = ring();
    let err = stopped(UNSEEDED_TC, &g, &capped(CAP));
    let RulesError::Budget { predicate, source, line, rows, cap } = &err else {
        panic!("expected a budget error, got {err:?}")
    };
    assert_eq!((predicate.as_str(), source.as_str(), *line, *cap), ("tc/2", "rules", 2, CAP));
    assert!(*rows > CAP && *rows < 160_000, "{rows}: stopped past the cap, before the closure");
    let text = err.to_string();
    assert!(text.starts_with("rule evaluation stopped: `tc/2` (rules:2) reached "), "{text}");
    // Actionable: the message names the seeded forms.
    assert!(text.contains("`tc(x, y) :- seed(x), ...`"), "{text}");
    assert!(text.contains("reaches(x, y, \"CATS\")"), "{text}");
}

#[test]
fn seeded_forms_stay_under() {
    let g = ring();
    let text = r#"hit(b) :- seed(a), reaches(a, b, "CALLS").
seed(a) :- node(a, _), index(a, 0).
stc(x, y) :- seed(x), edge(x, y, "CALLS").
stc(x, z) :- stc(x, y), edge(y, z, "CALLS").
pair(x, y) :- stc(x, y).
"#;
    let ev = evaluated(text, &g, &capped(CAP));
    assert_eq!(ev.outputs(), ["hit/1", "pair/2"]);
    // The ring: the seed reaches every node, itself included.
    assert_eq!(rendered(&ev, "hit"), every_node());
    let pairs = rendered(&ev, "pair");
    assert_eq!(pairs.len(), 400);
    assert!(pairs.iter().all(|p| p.starts_with("n00000 ")), "{:?}", pairs.first());
    assert!(ev.stats().derived_rows < CAP / 10, "{:?}", ev.stats());
}

#[test]
fn magic_seeds_user_recursion() {
    let g = ring();
    let text =
        format!("seed(a) :- node(a, _), index(a, 0).\n{UNSEEDED_TC}out(b) :- seed(a), tc(a, b).\n");
    let mut on = capped(CAP);
    on.outputs = vec!["out".into()];
    let ev = evaluated(&text, &g, &on);
    assert_eq!(ev.outputs(), ["out/1"]);
    assert_eq!(rendered(&ev, "out"), every_node());
    assert_eq!(ev.stats().magic_rewrites, 1, "{:?}", ev.stats());
    assert!(ev.stats().derived_rows < 2_000, "{:?}", ev.stats());

    // Without the rewrite the same question is the 160,000-row closure.
    let mut off = on.clone();
    off.magic = false;
    let err = stopped(&text, &g, &off);
    let RulesError::Budget { predicate, line, cap, .. } = &err else {
        panic!("expected a budget error, got {err:?}")
    };
    assert_eq!((predicate.as_str(), *line, *cap), ("tc/2", 3, CAP), "{err}");
}

#[test]
fn one_round_explosion() {
    let spec = GraphSpec {
        nodes: 400,
        edges: 0,
        categories: vec![],
        tier_mix: [100, 0, 0],
        seed: 1,
        salt: 0,
    };
    let g = lcg_graph(&spec);
    // One non-recursive round of 160,000 raw rows: counted as emitted, so it stops
    // before the round is inserted.
    let err = stopped("p(x, y) :- node(x, _), node(y, _).", &g, &capped(CAP));
    let RulesError::Budget { predicate, source, line, rows, cap } = &err else {
        panic!("expected a budget error, got {err:?}")
    };
    assert_eq!((predicate.as_str(), source.as_str(), *line, *cap), ("p/2", "rules", 1, CAP));
    assert!(*rows > CAP && *rows < 160_000, "{rows}");
    assert!(err.to_string().starts_with("rule evaluation stopped: `p/2` (rules:1) reached "));
    // Under a budget that holds it (160,000 bindings emitted, then 160,000 rows
    // inserted), the round is all 160,000 rows.
    let ev = evaluated("p(x, y) :- node(x, _), node(y, _).", &g, &capped(320_000));
    assert_eq!(ev.rows("p").map(|r| r.len()).ok(), Some(160_000));
}

#[test]
fn default_budget() {
    assert_eq!(Options::default().max_rows, DEFAULT_MAX_ROWS);
    assert_eq!(DEFAULT_MAX_ROWS, 5_000_000);
    assert!(Options::default().magic);
    assert_eq!(ExplainLimits::default().max_candidates, DEFAULT_MAX_ROWS);
    // The evaluation's own budget is the explain cap it offers.
    let g = ring();
    let ev = evaluated("p(x) :- node(x, _).", &g, &capped(CAP * 4));
    assert_eq!(ev.explain_limits().max_candidates, CAP * 4);
}

#[test]
fn explain_candidates_budget() {
    // two(n00000, n00051) has 50 equal-height derivations, one through each of
    // n00001..n00050.
    let spec = GraphSpec {
        nodes: 52,
        edges: 0,
        categories: vec![],
        tier_mix: [100, 0, 0],
        seed: 1,
        salt: 0,
    };
    let mut g = lcg_graph(&spec);
    let calls = category_id("CALLS");
    let id = |i: u32| -> NodeId { node_id(0, i) };
    for b in 1..=50 {
        g.edge(id(0), id(b), calls, Confidence::Strong, Tier::Fact);
        g.edge(id(b), id(51), calls, Confidence::Strong, Tier::Fact);
    }
    let text = r#"two(a, c) :- edge(a, b, "CALLS"), edge(b, c, "CALLS")."#;
    let mut ev = evaluated(text, &g, &Options::default());
    let rows = ev.rows("two").unwrap_or_else(|e| panic!("{e}"));
    let row = rows
        .iter()
        .find(|r| r.values == [Value::Node(id(0)), Value::Node(id(51))])
        .cloned()
        .unwrap_or_else(|| panic!("no two(n00000, n00051) row"));
    assert_eq!(rows.len(), 1, "every two-hop path joins n00000 to n00051");

    let mut limits = ExplainLimits::default();
    limits.max_candidates = 10;
    let err = ev.explain(&row, &limits).map(|w| w.height);
    assert_eq!(
        err,
        Err(RulesError::Budget {
            predicate: "two/2".into(),
            source: "rules".into(),
            line: 1,
            rows: 11,
            cap: 10
        })
    );
    if let Err(e) = &err {
        assert!(e.to_string().starts_with("rule evaluation stopped: `two/2` (rules:1) reached "));
    }
    // Under the evaluation's own cap the smallest rendered body wins: through n00001.
    let own = ev.explain_limits();
    let w = ev.explain(&row, &own).unwrap_or_else(|e| panic!("{e}"));
    let hops: Vec<(String, String)> = w
        .leaf_edges()
        .iter()
        .map(|h| (ev.render(&Value::Node(h.from)), ev.render(&Value::Node(h.to))))
        .collect();
    let want = [("n00000", "n00001"), ("n00001", "n00051")];
    assert_eq!(hops, want.map(|(a, b)| (a.to_string(), b.to_string())));
}
