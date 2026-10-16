//! DA.18: the composed evaluator (`eval::run`: parse, analyse, the magic rewrite,
//! plan, levelled semi-naive rounds, the closure builtins, aggregates, explain)
//! against independent oracles. Each module's unit tests check its own contract; this
//! target checks what they compose to, on testkit's randomized graphs and on three
//! hand-built ones, after LD.15a's pattern of holding an independent walk as the
//! oracle (activation/src/algo/reach.rs `mod oracle`).
//!
//! **The graphs.** [`graph`]`(seed)` is `testkit::lcg_graph` over seeds 1..=5: 300
//! nodes, 900 edges over CALLS / USES / IMPORTS at 60 / 30 / 10 percent FACT /
//! DERIVED / HEURISTIC.
//!
//! **The oracles.** [`Oracle`] holds, per level L, the [`Filtered`] graph (the nodes,
//! and the edges of one category set whose tier is <= L), its `Adjacency` and each
//! node's successors there. `tc(x, y)` holds at L iff `y` is in
//! `reach::reachable(adj_L, successors_L(x))`: the one-or-more-hop closure, so
//! `(x, x)` holds iff `x` is on a cycle. A row's tier is the smallest such L, and its
//! shortest hop count at L is the `reach::bfs` depth from those successors, plus one.
//! scc membership is `cycles::strongly_connected` over every tier; a member's tier is
//! the smallest L at which it shares a component with its representative. Negation is
//! a set difference and an aggregate a fold over the edges or the nodes.
//!
//! **Witnesses.** A `tc` witness's leaf edges must chain from x to y over graph edges
//! of tier <= the row's, as many as its height. (9) holds that count to the BFS depth
//! at the row's tier over graph 1's first 500 rows; (10) holds it, over every row of
//! every graph, to [`Oracle::lattice_heights`]: the lowest height of a derivation at
//! the row's level with each lower-level prefix at its own height, which is what
//! eval.rs records (its "Minimal heights"). The two differ where a lower-level prefix
//! row is longer than this level's path through it, since a row keeps one height, not
//! one per level: at DA.18, 99 of graph 4's 3,812 rows (`tc(n00031, n00050)`, DERIVED,
//! 8 hops through a 7-hop FACT `tc(n00031, n00006)` that DERIVED edges reach in 3; BFS
//! depth 4).
//!
//! Rows compare rendered (`Evaluation::render`), in `rows()` order, with their tiers.
//! `Options` and `ExplainLimits` are `#[non_exhaustive]`, so they are built with
//! `Default` and field assignment. Every evaluation prints its `[rules] eval` line,
//! and the magic cases `[rules] magic pred=<p>/2 adornment=bf sites=1`, on stderr
//! (visible with `--nocapture`).

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

use glia_activation::algo::cycles::strongly_connected;
use glia_activation::algo::reach::{bfs, reachable};
use glia_activation::algo::{Adjacency, CategorySet, GraphSource, Walk};
use glia_core::{Confidence, Edge, EdgeCategoryId, NodeId, NodeKindId};
use glia_rules::eval::{Evaluation, Options, Row, run};
use glia_rules::explain::{EdgeHop, ExplainLimits};
use glia_rules::source::{FactSource, MemSource};
use glia_rules::testkit::{self, GraphSpec};
use glia_rules::value::{Tier, Value};

/// Left-linear transitive closure over CALLS.
const TC: &str = r#"tc(x, y) :- edge(x, y, "CALLS").
tc(x, z) :- tc(x, y), edge(y, z, "CALLS").
"#;

/// The closure builtin over the same category, one walk per bound source.
const REACHES: &str = r#"r(x, y) :- node(x, _), reaches(x, y, "CALLS")."#;

/// The randomized graphs' seeds.
const SEEDS: std::ops::RangeInclusive<u64> = 1..=5;

/// A row as compared: its rendered values and its tier.
type Rendered = (Vec<String>, Tier);

fn graph(seed: u64) -> MemSource {
    testkit::lcg_graph(&GraphSpec {
        nodes: 300,
        edges: 900,
        categories: vec!["CALLS", "USES", "IMPORTS"],
        tier_mix: [60, 30, 10],
        seed,
        salt: 0,
    })
}

fn opts(magic: bool) -> Options {
    let mut o = Options::default();
    o.magic = magic;
    o
}

fn eval<'s>(text: &str, g: &'s MemSource, o: &Options) -> Evaluation<'s, MemSource> {
    run("oracle", text, g, o).unwrap_or_else(|e| panic!("{text}\n{e}"))
}

fn rows(ev: &Evaluation<'_, MemSource>, pred: &str) -> Vec<Row> {
    ev.rows(pred).unwrap_or_else(|e| panic!("rows({pred}): {e}"))
}

fn rendered(ev: &Evaluation<'_, MemSource>, pred: &str) -> Vec<Rendered> {
    rows(ev, pred)
        .iter()
        .map(|r| (r.values.iter().map(|v| ev.render(v)).collect(), r.tier))
        .collect()
}

/// The kind name of `id` (testkit's registry).
fn kind_name(g: &MemSource, id: NodeId) -> &'static str {
    g.node_kind(id).and_then(|k| g.tables().registries.kind_name(k)).unwrap_or("?")
}

/// The nodes of kind `kind`, in node order.
fn of_kind(g: &MemSource, kind: &str) -> Vec<NodeId> {
    g.node_ids().into_iter().filter(|id| kind_name(g, *id) == kind).collect()
}

/// Node rows rendered and sorted as `rows()` sorts them: column by column, a node by
/// its render (the renders here are unique, and testkit's zero-padded, so string order
/// is `rows()` order).
fn sorted(g: &MemSource, rows: Vec<(Vec<NodeId>, Tier)>) -> Vec<Rendered> {
    let mut out: Vec<Rendered> = rows
        .into_iter()
        .map(|(ids, t)| (ids.iter().map(|id| g.render_node(*id)).collect(), t))
        .collect();
    out.sort();
    out
}

// ============================================================================
// The oracle
// ============================================================================

/// A source's nodes and the edges of one category set at tier <= one level.
struct Filtered {
    nodes: Vec<NodeId>,
    edges: Vec<Edge>,
}

impl Filtered {
    fn new(g: &MemSource, cats: &[EdgeCategoryId], level: Tier) -> Self {
        let edges = g
            .edges()
            .enumerate()
            .filter(|(i, e)| cats.contains(&e.category) && g.edge_tier(*i, e) <= level)
            .map(|(_, e)| e.clone())
            .collect();
        Filtered { nodes: g.node_ids(), edges }
    }
}

impl GraphSource for Filtered {
    fn node_ids(&self) -> Vec<NodeId> {
        self.nodes.clone()
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(self.edges.iter())
    }
}

/// One level's walk index and successor lists.
struct Level {
    adj: Adjacency,
    succ: HashMap<NodeId, Vec<NodeId>>,
}

/// The closure oracle of one category set over one graph, per level.
struct Oracle {
    nodes: Vec<NodeId>,
    levels: Vec<Level>,
}

impl Oracle {
    fn new(g: &MemSource, cats: &[&str]) -> Self {
        let ids: Vec<EdgeCategoryId> = cats.iter().map(|c| testkit::category_id(c)).collect();
        let levels = Tier::ALL
            .iter()
            .map(|&level| {
                let f = Filtered::new(g, &ids, level);
                let adj = Adjacency::build(&f, &CategorySet::all());
                let mut succ: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
                for e in &f.edges {
                    succ.entry(e.from).or_default().push(e.to);
                }
                Level { adj, succ }
            })
            .collect();
        Oracle { nodes: g.node_ids(), levels }
    }

    fn succ(&self, level: Tier, x: NodeId) -> &[NodeId] {
        self.levels[usize::from(level.level())].succ.get(&x).map_or(&[], Vec::as_slice)
    }

    /// What `x` reaches in one or more hops at `level`.
    fn reach(&self, level: Tier, x: NodeId) -> HashSet<NodeId> {
        let s = self.succ(level, x);
        if s.is_empty() {
            return HashSet::new();
        }
        reachable(&self.levels[usize::from(level.level())].adj, s, Walk::Forward)
    }

    /// The tier of `(x, y)` for every `y` that `x` reaches.
    fn tiers_from(&self, x: NodeId) -> HashMap<NodeId, Tier> {
        let mut out = HashMap::new();
        for level in Tier::ALL {
            for y in self.reach(level, x) {
                out.entry(y).or_insert(level);
            }
        }
        out
    }

    /// The shortest hop count (>= 1) from `x` to each node it reaches at `level`.
    fn depths(&self, level: Tier, x: NodeId) -> HashMap<NodeId, usize> {
        let s = self.succ(level, x);
        let mut out: HashMap<NodeId, usize> = s.iter().map(|&y| (y, 1)).collect();
        let walk = bfs(&self.levels[usize::from(level.level())].adj, s, Walk::Forward, usize::MAX);
        for r in walk.reached {
            out.entry(r.id).or_insert(r.depth + 1);
        }
        out
    }

    /// Every `(x, y)` pair of the closure with its tier.
    fn tc(&self) -> Vec<(Vec<NodeId>, Tier)> {
        let mut out = Vec::new();
        for &x in &self.nodes {
            out.extend(self.tiers_from(x).into_iter().map(|(y, t)| (vec![x, y], t)));
        }
        out
    }

    /// Each `y` that `x` reaches, with the tier of `(x, y)` and the lowest height of a
    /// derivation of left-linear `tc(x, y)` at that tier: an edge `x -> y` is height 1
    /// and `tc(x, p)` plus an edge `p -> y` is `tc(x, p)`'s height plus 1, over edges of
    /// tier <= the row's, with a lower-tier `tc(x, p)` read at its own height. Per
    /// level, a Dijkstra over unit hops seeded at those heights.
    fn lattice_heights(&self, x: NodeId) -> HashMap<NodeId, (Tier, usize)> {
        let mut known: HashMap<NodeId, (Tier, usize)> = HashMap::new();
        for level in Tier::ALL {
            let mut heap: BinaryHeap<Reverse<(usize, u64)>> = BinaryHeap::new();
            heap.extend(self.succ(level, x).iter().map(|y| Reverse((1, y.0))));
            for (&p, &(_, h)) in &known {
                heap.extend(self.succ(level, p).iter().map(|y| Reverse((h + 1, y.0))));
            }
            let mut found: HashMap<NodeId, usize> = HashMap::new();
            while let Some(Reverse((h, y))) = heap.pop() {
                let y = NodeId(y);
                if known.contains_key(&y) || found.contains_key(&y) {
                    continue;
                }
                found.insert(y, h);
                heap.extend(self.succ(level, y).iter().map(|z| Reverse((h + 1, z.0))));
            }
            known.extend(found.into_iter().map(|(y, h)| (y, (level, h))));
        }
        known
    }

    /// Every `y` some source reaches, at the best tier over the sources.
    fn reached_from(&self, sources: &[NodeId]) -> HashMap<NodeId, Tier> {
        let mut out: HashMap<NodeId, Tier> = HashMap::new();
        for &a in sources {
            for (y, t) in self.tiers_from(a) {
                let best = out.entry(y).or_insert(t);
                *best = (*best).min(t);
            }
        }
        out
    }
}

// ============================================================================
// (1)-(4): the closure, as rules and as builtins
// ============================================================================

#[test]
fn tc_rules_match_reachable_per_level() {
    for seed in SEEDS {
        let g = graph(seed);
        let oracle = Oracle::new(&g, &["CALLS"]);
        let ev = eval(TC, &g, &Options::default());
        let got = rendered(&ev, "tc");
        assert_eq!(got, sorted(&g, oracle.tc()), "seed {seed}");
        for t in Tier::ALL {
            assert!(got.iter().any(|r| r.1 == t), "seed {seed}: no {t:?} row");
        }
        // (x, x) holds exactly on a cycle: the closure's off-by-one, pinned by Tarjan.
        let on_cycle = strongly_connected(&oracle.levels[2].adj).concat().len();
        let diagonal = got.iter().filter(|(v, _)| v[0] == v[1]).count();
        assert_eq!(diagonal, on_cycle, "seed {seed}");
        assert!(diagonal > 0, "seed {seed}: no CALLS cycle to pin (x, x) on");
    }
}

#[test]
fn reaches_builtin_matches_tc() {
    for seed in SEEDS {
        let g = graph(seed);
        let tc = rendered(&eval(TC, &g, &Options::default()), "tc");
        let r = rendered(&eval(REACHES, &g, &Options::default()), "r");
        assert!(!r.is_empty(), "seed {seed}");
        assert_eq!(r, tc, "seed {seed}");
    }
}

#[test]
fn hoisted_equals_per_source() {
    // `a` is dead after the call, so the planner folds the ROUTE scan into one
    // multi-source walk (ReachMulti); `pair` keeps `a`, so it walks per source.
    let hoisted = r#"hit(b) :- node(a, "ROUTE"), reaches(a, b, "CALLS")."#;
    let per_source = r#"pair(a, b) :- node(a, "ROUTE"), reaches(a, b, "CALLS").
hit2(b) :- pair(_, b).
"#;
    for seed in SEEDS {
        let g = graph(seed);
        let hit = rendered(&eval(hoisted, &g, &Options::default()), "hit");
        let hit2 = rendered(&eval(per_source, &g, &Options::default()), "hit2");
        assert_eq!(hit, hit2, "seed {seed}");
        let oracle = Oracle::new(&g, &["CALLS"]);
        let want = oracle.reached_from(&of_kind(&g, "ROUTE"));
        let want = sorted(&g, want.into_iter().map(|(y, t)| (vec![y], t)).collect());
        assert!(!want.is_empty(), "seed {seed}");
        assert_eq!(hit, want, "seed {seed}");
    }
}

#[test]
fn scc_matches_tarjan() {
    for seed in SEEDS {
        let g = graph(seed);
        let oracle = Oracle::new(&g, &["CALLS"]);
        // Per level: each member's component number.
        let component: Vec<HashMap<NodeId, usize>> = oracle
            .levels
            .iter()
            .map(|l| {
                let comps = strongly_connected(&l.adj);
                let mut of = HashMap::new();
                for (c, members) in comps.iter().enumerate() {
                    of.extend(members.iter().map(|&m| (m, c)));
                }
                of
            })
            .collect();
        let mut want = Vec::new();
        for members in strongly_connected(&oracle.levels[2].adj) {
            let rep = members
                .iter()
                .copied()
                .min_by_key(|id| g.render_node(*id))
                .expect("a component has members");
            for &x in &members {
                let tier = Tier::ALL
                    .into_iter()
                    .find(|&t| {
                        let of = &component[usize::from(t.level())];
                        of.get(&x).is_some_and(|c| of.get(&rep) == Some(c))
                    })
                    .expect("every member shares its component with rep at HEURISTIC");
                want.push((vec![x, rep], tier));
            }
        }
        assert!(!want.is_empty(), "seed {seed}: no CALLS cycle");
        let ev = eval(r#"s(x, rep) :- scc(x, rep, "CALLS")."#, &g, &Options::default());
        assert_eq!(rendered(&ev, "s"), sorted(&g, want), "seed {seed}");
    }
}

// ============================================================================
// (5): the tier lattice (R1 section 9, spike 3)
// ============================================================================

/// A hand-built edge: from, to (by name) and tier.
type NamedEdge<'a> = (&'a str, &'a str, Tier);

/// `names` as FUNCTION nodes `0..` rendered by name, then `edges` over CALLS, in
/// order.
fn named(names: &[&str], edges: &[NamedEdge<'_>]) -> MemSource {
    let mut g = MemSource::new(testkit::tables());
    let kind = NodeKindId(testkit::KINDS[0].1);
    let id = |n: &str| {
        let i = names.iter().position(|x| *x == n).expect("a named node");
        testkit::node_id(0, i as u32)
    };
    for n in names {
        g.node(id(n), kind, *n);
    }
    let strength = |t: Tier| match t {
        Tier::Fact => Confidence::Strong,
        Tier::Derived => Confidence::Medium,
        Tier::Heuristic => Confidence::Weak,
    };
    for &(from, to, tier) in edges {
        g.edge(id(from), id(to), testkit::category_id("CALLS"), strength(tier), tier);
    }
    g
}

/// Each leaf edge as `from->to tier`.
fn hops(g: &MemSource, hops: &[EdgeHop]) -> Vec<String> {
    hops.iter()
        .map(|h| format!("{}->{} {}", g.render_node(h.from), g.render_node(h.to), h.tier.name()))
        .collect()
}

/// The row of `pred` whose values render `want`.
fn row_of(ev: &Evaluation<'_, MemSource>, pred: &str, want: &[&str]) -> Row {
    rows(ev, pred)
        .into_iter()
        .find(|r| r.values.iter().map(|v| ev.render(v)).eq(want.iter().map(|s| s.to_string())))
        .unwrap_or_else(|| panic!("no {pred}{want:?}"))
}

#[test]
fn tier_lattice_fixture() {
    use Tier::{Derived as D, Fact as F, Heuristic as H};
    let names = ["a", "b", "c", "d", "e"];
    // The HEURISTIC hop comes first in edge order: a first-found walk takes it.
    let short = [("a", "d", H)];
    let fact = [("a", "b", F), ("b", "c", F), ("c", "d", F)];
    let derived = [("a", "e", D), ("e", "d", D)];
    let cases: [(Vec<NamedEdge<'_>>, Tier, &[&str]); 3] = [
        (
            [&short[..], &fact[..], &derived[..]].concat(),
            F,
            &["a->b fact", "b->c fact", "c->d fact"],
        ),
        ([&short[..], &derived[..]].concat(), D, &["a->e derived", "e->d derived"]),
        (short.to_vec(), H, &["a->d heuristic"]),
    ];
    let text = format!("{TC}{REACHES}\n");
    for (edges, tier, want) in cases {
        let g = named(&names, &edges);
        let mut ev = eval(&text, &g, &Options::default());
        for pred in ["tc", "r"] {
            let row = row_of(&ev, pred, &["a", "d"]);
            assert_eq!(row.tier, tier, "{pred}(a, d) over {edges:?}");
            let w = ev
                .explain(&row, &ExplainLimits::default())
                .unwrap_or_else(|e| panic!("explain {pred}(a, d): {e}"));
            assert_eq!(w.tier, tier, "{pred}");
            assert_eq!(hops(&g, &w.leaf_edges()), want, "{pred}(a, d) over {edges:?}");
        }
    }
}

// ============================================================================
// (6)-(8): magic, negation, aggregates
// ============================================================================

#[test]
fn magic_preserves_answers() {
    // Each closure read from one bound site: the ten nodes of index below 10.
    let site = |p: &str| format!("out(b) :- index(a, i), i < 10, {p}(a, b).\n");
    let left = format!("{TC}{}", site("tc"));
    let right = format!(
        "tc(x, y) :- edge(x, y, \"CALLS\").\ntc(x, z) :- edge(x, y, \"CALLS\"), tc(y, z).\n{}",
        site("tc")
    );
    let same_generation = format!(
        "sg(x, y) :- edge(x, y, \"IMPORTS\").\n\
         sg(x, y) :- edge(x, u, \"USES\"), sg(u, w), edge(y, w, \"USES\").\n{}",
        site("sg")
    );
    let mut sg_rows = 0;
    for seed in SEEDS {
        let g = graph(seed);
        let seeds: Vec<NodeId> = (0..10).map(|i| testkit::node_id(0, i)).collect();
        let oracle = Oracle::new(&g, &["CALLS"]);
        let reached = oracle.reached_from(&seeds);
        let want = sorted(&g, reached.into_iter().map(|(y, t)| (vec![y], t)).collect());
        assert!(!want.is_empty(), "seed {seed}");
        for (name, text) in [("left", &left), ("right", &right), ("sg", &same_generation)] {
            let on = eval(text, &g, &opts(true));
            let off = eval(text, &g, &opts(false));
            assert_eq!(on.stats().magic_rewrites, 1, "{name} seed {seed}: no rewrite");
            assert_eq!(off.stats().magic_rewrites, 0, "{name} seed {seed}");
            let rows_on = rendered(&on, "out");
            assert_eq!(rows_on, rendered(&off, "out"), "{name} seed {seed}");
            match name {
                "sg" => sg_rows += rows_on.len(),
                _ => assert_eq!(rows_on, want, "{name} seed {seed}"),
            }
            if name == "left" {
                let (with, without) = (on.stats().derived_rows, off.stats().derived_rows);
                assert!(
                    with * 2 < without,
                    "seed {seed}: {with} rows with magic, {without} without"
                );
            }
        }
    }
    assert!(sg_rows > 0, "the same-generation site answers nothing on any graph");
}

#[test]
fn negation_is_set_difference() {
    let text = r#"live(y) :- node(x, "ROUTE"), reaches(x, y).
dead(f) :- node(f, "FUNCTION"), !live(f).
"#;
    let mut o = Options::default();
    o.outputs = vec!["live".into(), "dead".into()];
    for seed in SEEDS {
        let g = graph(seed);
        // reaches/2 follows the carry set: testkit's is CALLS, USES.
        let oracle = Oracle::new(&g, &["CALLS", "USES"]);
        let live = oracle.reached_from(&of_kind(&g, "ROUTE"));
        let ev = eval(text, &g, &o);
        let want_live = sorted(&g, live.iter().map(|(y, t)| (vec![*y], *t)).collect());
        assert_eq!(rendered(&ev, "live"), want_live, "seed {seed}");
        let dead: Vec<(Vec<NodeId>, Tier)> = of_kind(&g, "FUNCTION")
            .into_iter()
            .filter(|f| !live.contains_key(f))
            .map(|f| (vec![f], Tier::Fact))
            .collect();
        assert!(!dead.is_empty(), "seed {seed}: every FUNCTION is live");
        assert_eq!(rendered(&ev, "dead"), sorted(&g, dead), "seed {seed}");
        for row in rows(&ev, "dead") {
            let [caveat] = row.absence.as_slice() else {
                panic!("seed {seed}: one caveat per dead row, got {:?}", row.absence);
            };
            assert_eq!(caveat.negated, "live/1");
            assert_eq!(caveat.categories, ["CALLS", "USES"]);
            assert!(!caveat.any_category);
        }
    }
}

#[test]
fn aggregates_match_folds() {
    let text = r#"fan(y, count(x)) :- edge(x, y, "CALLS").
hi(k, max(i)) :- node(x, k), index(x, i).
tot(k, sum(i)) :- node(x, k), index(x, i).
"#;
    for seed in SEEDS {
        let g = graph(seed);
        let ev = eval(text, &g, &Options::default());
        // fan: distinct callers per callee; a pair's tier is its best parallel edge, a
        // group's the weakest of its pairs.
        let calls = testkit::category_id("CALLS");
        let mut pairs: HashMap<(NodeId, NodeId), Tier> = HashMap::new();
        for (i, e) in g.edges().enumerate().filter(|(_, e)| e.category == calls) {
            let t = g.edge_tier(i, e);
            let best = pairs.entry((e.from, e.to)).or_insert(t);
            *best = (*best).min(t);
        }
        let mut fan: HashMap<NodeId, (i64, Tier)> = HashMap::new();
        for ((_, y), t) in &pairs {
            let group = fan.entry(*y).or_insert((0, Tier::Fact));
            *group = (group.0 + 1, group.1.max(*t));
        }
        let mut want: Vec<Rendered> =
            fan.into_iter().map(|(y, (n, t))| (vec![g.render_node(y), n.to_string()], t)).collect();
        want.sort();
        assert_eq!(rendered(&ev, "fan"), want, "seed {seed}");
        // hi / tot: per kind name, the max and the sum of `index`; nodes and
        // attributes are FACT.
        let mut kinds: Vec<(&str, i64, i64)> = Vec::new();
        for (i, id) in g.node_ids().into_iter().enumerate() {
            let (k, i) = (kind_name(&g, id), i as i64);
            match kinds.iter_mut().find(|(name, ..)| *name == k) {
                Some(row) => *row = (k, row.1.max(i), row.2 + i),
                None => kinds.push((k, i, i)),
            }
        }
        kinds.sort();
        let fold = |pick: fn(&(&str, i64, i64)) -> i64| -> Vec<Rendered> {
            kinds.iter().map(|r| (vec![r.0.to_string(), pick(r).to_string()], Tier::Fact)).collect()
        };
        assert_eq!(rendered(&ev, "hi"), fold(|r| r.1), "seed {seed}");
        assert_eq!(rendered(&ev, "tot"), fold(|r| r.2), "seed {seed}");
    }
}

// ============================================================================
// (9)-(11): witnesses: valid paths of the row's tier, of the recorded (lowest) height
// ============================================================================

/// Check tc row `row`'s witness: its leaf edges chain from x to y, each an edge of
/// the graph (endpoints, category and tier as stored at its ordinal) at most the
/// row's tier, and there are exactly `want(x, y)` of them, the witness height.
fn check_witness(
    g: &MemSource,
    ev: &mut Evaluation<'_, MemSource>,
    row: &Row,
    want: &dyn Fn(NodeId, NodeId) -> Option<usize>,
) {
    let edges: Vec<(&Edge, Tier)> =
        g.edges().enumerate().map(|(i, e)| (e, g.edge_tier(i, e))).collect();
    let [Value::Node(x), Value::Node(y)] = row.values.as_slice() else {
        panic!("a tc row holds two nodes: {:?}", row.values);
    };
    let w = ev.explain(row, &ExplainLimits::default()).unwrap_or_else(|e| panic!("{e}"));
    let path = w.leaf_edges();
    let label = format!("tc({}, {}) {:?}", g.render_node(*x), g.render_node(*y), row.tier);
    assert!(!w.truncated, "{label}");
    assert_eq!(w.tier, row.tier, "{label}");
    assert_eq!(path.first().map(|h| h.from), Some(*x), "{label}: {:?}", hops(g, &path));
    assert_eq!(path.last().map(|h| h.to), Some(*y), "{label}: {:?}", hops(g, &path));
    for pair in path.windows(2) {
        assert_eq!(pair[0].to, pair[1].from, "{label}: {:?}", hops(g, &path));
    }
    for h in &path {
        let (e, t) = edges[h.ordinal];
        assert_eq!((e.from, e.to, e.category, t), (h.from, h.to, h.category, h.tier), "{label}");
        assert_eq!(h.category, testkit::category_id("CALLS"), "{label}");
        assert!(h.tier <= row.tier, "{label}: hop at {:?}", h.tier);
    }
    assert_eq!(Some(path.len()), want(*x, *y), "{label}: {:?}", hops(g, &path));
    assert_eq!(w.height as usize, path.len(), "{label}");
}

#[test]
fn witnesses_are_valid_shortest_paths() {
    let g = graph(1);
    let oracle = Oracle::new(&g, &["CALLS"]);
    let mut ev = eval(TC, &g, &Options::default());
    let all = rows(&ev, "tc");
    assert!(all.len() >= 500, "graph 1 has {} tc rows", all.len());
    for t in Tier::ALL {
        assert!(all[..500].iter().any(|r| r.tier == t), "no {t:?} row among the first 500");
    }
    for row in &all[..500] {
        let bfs_depth = |x: NodeId, y: NodeId| oracle.depths(row.tier, x).get(&y).copied();
        check_witness(&g, &mut ev, row, &bfs_depth);
    }
}

#[test]
fn witness_heights_match_the_lattice_oracle() {
    // Every tc row of every graph: its witness height is the lowest height of a
    // derivation at its level, a lower-level prefix read at its own height.
    for seed in SEEDS {
        let g = graph(seed);
        let oracle = Oracle::new(&g, &["CALLS"]);
        let mut ev = eval(TC, &g, &Options::default());
        let mut heights: HashMap<NodeId, HashMap<NodeId, (Tier, usize)>> = HashMap::new();
        for row in rows(&ev, "tc") {
            let Value::Node(x) = row.values[0] else { panic!("{:?}", row.values) };
            let from_x = heights.entry(x).or_insert_with(|| {
                let (h, tiers) = (oracle.lattice_heights(x), oracle.tiers_from(x));
                assert_eq!(h.len(), tiers.len(), "seed {seed}");
                assert!(h.iter().all(|(y, (t, _))| tiers.get(y) == Some(t)), "seed {seed}");
                h
            });
            let lattice = |_: NodeId, y: NodeId| {
                from_x.get(&y).filter(|(t, _)| *t == row.tier).map(|(_, h)| *h)
            };
            check_witness(&g, &mut ev, &row, &lattice);
        }
    }
}

#[test]
fn same_level_rederivation_keeps_the_shortest_witness() {
    // DA.15's counterexample. At DERIVED, tc(a, v) is first derived through the long
    // FACT prefix a -> q1 .. w (6 hops) and only in a later round through a -> u
    // (2 hops); tc(a, z) first through a -> p1 -> p2 -> y (4 hops), later through v
    // (3 hops). The shortest DERIVED path is a -> u -> v -> z.
    use Tier::{Derived as D, Fact as F};
    let names = ["a", "p1", "p2", "y", "z", "u", "v", "q1", "q2", "q3", "q4", "w"];
    let edges = [
        ("a", "p1", F),
        ("p1", "p2", F),
        ("p2", "y", F),
        ("y", "z", D),
        ("a", "u", D),
        ("u", "v", F),
        ("v", "z", F),
        ("a", "q1", F),
        ("q1", "q2", F),
        ("q2", "q3", F),
        ("q3", "q4", F),
        ("q4", "w", F),
        ("w", "v", D),
    ];
    let g = named(&names, &edges);
    let oracle = Oracle::new(&g, &["CALLS"]);
    let mut ev = eval(TC, &g, &Options::default());
    let row = row_of(&ev, "tc", &["a", "z"]);
    assert_eq!(row.tier, D);
    let w = ev.explain(&row, &ExplainLimits::default()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(hops(&g, &w.leaf_edges()), ["a->u derived", "u->v fact", "v->z fact"]);
    for row in rows(&ev, "tc") {
        let bfs_depth = |x: NodeId, y: NodeId| oracle.depths(row.tier, x).get(&y).copied();
        check_witness(&g, &mut ev, &row, &bfs_depth);
    }
}
