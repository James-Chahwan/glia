//! DA.19: the determinism gate (plan section 2 DA; R1 section 9 spike 5, run
//! in-process). Code NodeIds hash a path-derived RepoId and the Finalize sort orders
//! edges by id, so a checkout at another path is the same rendered graph under other
//! ids in another edge order. `testkit::resalted` builds exactly that, and rows, tiers,
//! absence caveats and rendered witnesses must not move with it.
//!
//! - [`same_rows_across_checkouts`]: every output of [`PROGRAM`] over [`graph`] and
//!   its resalted copy, compared rendered (`Evaluation::render` per value, plus tier
//!   and absence caveats), with the warnings and the counters;
//! - [`same_witnesses_across_checkouts`]: the rendered witness trees and leaf hops of
//!   `tc`, `hit` and `cyc` rows (the first 200 in rendered order, plus 200 spread over
//!   the rest), never their ordinals, which legitimately differ;
//! - [`repeat_runs_identical`]: two evaluations of one graph give equal `Row`s (handles
//!   and provenance included), equal stats apart from `elapsed_ms` and equal
//!   witnesses, ordinals included;
//! - [`source_guard`]: a text scan of `rules/src`, walked recursively at run time so a
//!   file a later packet adds is covered too (module doc of the test).

use std::fs;
use std::path::{Path, PathBuf};

use glia_activation::algo::GraphSource;
use glia_core::{EdgeCategoryId, NodeId};
use glia_rules::eval::{Evaluation, Options, Row, run};
use glia_rules::explain::{EdgeHop, ExplainLimits, Leaf, Witness};
use glia_rules::ir::{AttrDecl, ColType};
use glia_rules::source::{FactSource, MemSource};
use glia_rules::testkit::{GraphSpec, lcg_graph, resalted, tables};
use glia_rules::value::{Tier, Value};

/// One program over every evaluator path: seeded recursion (`tc`), a negation with an
/// absence caveat (`unhandled`, `named`), one aggregate per op (`fan_in` count, `lo`
/// min, `hi` max, `total` sum), a hoisted multi-source `reaches` (`far`: the source is
/// projected away) and a per-source one (`hit`), `scc` (`cyc`), comparisons on the
/// `index` attribute, `starts_with` on the rendered-name attribute `name`, and an
/// unseeded recursion read from a bound site, which the magic rewrite seeds (`link`
/// read by `out`).
const PROGRAM: &str = r#"// DA.19's determinism program.
tc(x, y) :- edge(x, y, "CALLS"), index(x, i), i < 40.
tc(x, z) :- tc(x, y), edge(y, z, "CALLS").
handled(r) :- edge(r, _, "HANDLED_BY").
unhandled(r) :- node(r, "ROUTE"), !handled(r).
fan_in(y, count(x)) :- edge(x, y, "CALLS").
lo(x, min(j)) :- tc(x, y), index(y, j).
hi(x, max(j)) :- tc(x, y), index(y, j).
total(k, sum(i)) :- node(x, k), index(x, i).
far(b) :- node(a, "ROUTE"), index(a, i), i < 20, reaches(a, b, "CALLS|USES").
hit(a, b) :- node(a, "MODULE"), index(a, i), i >= 10, i < 90, i != 49, reaches(a, b, "CALLS|USES").
cyc(x, r) :- scc(x, r, "CALLS|USES").
named(x, y) :- tc(x, y), name(y, s), starts_with(s, "n001"), !handled(y).
seed(a) :- index(a, 3).
link(x, y) :- edge(x, y, "USES").
link(x, z) :- link(x, y), edge(y, z, "USES").
out(b) :- seed(a), link(a, b).
"#;

/// The outputs [`PROGRAM`] is evaluated for: every predicate but the helpers `handled`,
/// `seed` and `link` (so `link` is magic-rewritten; outputs never are).
const OUTPUTS: [&str; 11] =
    ["tc", "unhandled", "fan_in", "lo", "hi", "total", "far", "hit", "cyc", "named", "out"];

/// Attribute 1 of [`graph`]: `name(Node, str)`, a node's render (`n00042`).
const NAME: AttrDecl = AttrDecl { name: "name", ty: ColType::Str };

/// `lcg_graph` (seed 3, 400 nodes, 1,200 edges over CALLS / USES / HANDLED_BY at 60 /
/// 30 / 10 percent FACT / DERIVED / HEURISTIC), plus the [`NAME`] column.
fn graph() -> MemSource {
    let spec = GraphSpec {
        nodes: 400,
        edges: 1_200,
        categories: vec!["CALLS", "USES", "HANDLED_BY"],
        tier_mix: [60, 30, 10],
        seed: 3,
        salt: 0,
    };
    let mut g = lcg_graph(&spec);
    g.attr(NAME);
    for id in g.node_ids() {
        let render = g.render_node(id);
        g.set_attr(id, 1, Value::Str(render));
    }
    g
}

fn options() -> Options {
    let mut opts = Options::default();
    opts.outputs = OUTPUTS.iter().map(|s| s.to_string()).collect();
    opts
}

fn eval(g: &MemSource) -> Evaluation<'_, MemSource> {
    run("rules", PROGRAM, g, &options()).unwrap_or_else(|e| panic!("evaluation failed: {e}"))
}

fn rows<S: FactSource + ?Sized>(ev: &Evaluation<'_, S>, pred: &str) -> Vec<Row> {
    ev.rows(pred).unwrap_or_else(|e| panic!("rows({pred}): {e}"))
}

/// A row as a checkout-independent value: rendered values, tier, absence caveats.
type Rendered = (Vec<String>, Tier, Vec<(String, Vec<String>, bool)>);

fn rendered_rows<S: FactSource + ?Sized>(ev: &Evaluation<'_, S>, pred: &str) -> Vec<Rendered> {
    rows(ev, pred)
        .iter()
        .map(|r| {
            let values = r.values.iter().map(|v| ev.render(v)).collect();
            let absence = r
                .absence
                .iter()
                .map(|c| (c.negated.clone(), c.categories.clone(), c.any_category))
                .collect();
            (values, r.tier, absence)
        })
        .collect()
}

fn node<S: FactSource + ?Sized>(ev: &Evaluation<'_, S>, id: NodeId) -> String {
    ev.render(&Value::Node(id))
}

fn category(c: EdgeCategoryId) -> String {
    tables().registries.category_name(c).map_or_else(|| format!("#{}", c.0), str::to_string)
}

/// A hop by (render(from), render(to), category name, tier): never its ordinal.
fn hop<S: FactSource + ?Sized>(ev: &Evaluation<'_, S>, h: &EdgeHop) -> String {
    format!("{} -> {} {} {:?}", node(ev, h.from), node(ev, h.to), category(h.category), h.tier)
}

fn leaf<S: FactSource + ?Sized>(ev: &Evaluation<'_, S>, l: Option<&Leaf>) -> String {
    let Some(l) = l else { return "-".into() };
    match l {
        Leaf::Edge(h) => format!("edge[{}]", hop(ev, h)),
        Leaf::Path(hops) => {
            let hops: Vec<String> = hops.iter().map(|h| hop(ev, h)).collect();
            format!("path[{}]", hops.join(", "))
        }
        Leaf::Node { id, kind } => format!("node[{} {kind}]", node(ev, *id)),
        Leaf::Attr { id, name, value } => {
            format!("attr[{} {name} {}]", node(ev, *id), ev.render(value))
        }
        Leaf::Cell { id, cell_type } => format!("cell[{} {cell_type}]", node(ev, *id)),
        Leaf::Builtin { name, args } => {
            let args: Vec<String> = args.iter().map(|v| ev.render(v)).collect();
            format!("builtin[{name}({})]", args.join(", "))
        }
        Leaf::Registry => "registry".into(),
        other => format!("{other:?}"),
    }
}

/// The witness tree depth-first, one line per node: predicate, rendered values, tier,
/// height, rule, flags and leaf (hops rendered). An explicit stack, as explain's own
/// walks are.
fn rendered_witness<S: FactSource + ?Sized>(ev: &Evaluation<'_, S>, w: &Witness) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![(0usize, w)];
    while let Some((depth, w)) = stack.pop() {
        let values: Vec<String> = w.values.iter().map(|v| ev.render(v)).collect();
        let rule = w.rule.as_ref().map(|r| format!("{}:{} #{}", r.source, r.line, r.index));
        out.push(format!(
            "{:indent$}{}({}) {:?} h={} rule={} neg={} trunc={} {}",
            "",
            w.predicate,
            values.join(", "),
            w.tier,
            w.height,
            rule.unwrap_or_else(|| "-".into()),
            w.negated,
            w.truncated,
            leaf(ev, w.leaf.as_ref()),
            indent = depth * 2,
        ));
        stack.extend(w.support.iter().rev().map(|c| (depth + 1, c)));
    }
    out
}

/// The rows a witness test explains: the first 200 in rendered order, then 200 spread
/// evenly over the rest.
fn sample(rows: &[Row]) -> Vec<Row> {
    let mut picked: Vec<usize> = (0..rows.len().min(200)).collect();
    if rows.len() > 200 {
        let rest = rows.len() - 200;
        let step = rest.div_ceil(200).max(1);
        picked.extend((200..rows.len()).step_by(step));
    }
    picked.into_iter().map(|i| rows[i].clone()).collect()
}

#[test]
fn same_rows_across_checkouts() {
    let g = graph();
    let g2 = resalted(&g, 99);
    let (a, b) = (eval(&g), eval(&g2));
    let outputs = a.outputs();
    assert_eq!(outputs.len(), OUTPUTS.len(), "{outputs:?}");
    assert_eq!(b.outputs(), outputs);

    let mut tiers_seen: Vec<Tier> = Vec::new();
    for pred in &outputs {
        let (ra, rb) = (rendered_rows(&a, pred), rendered_rows(&b, pred));
        assert!(!ra.is_empty(), "{pred} has no rows: the gate would prove nothing");
        assert_eq!(ra.len(), rb.len(), "{pred}: row counts differ");
        for (i, (x, y)) in ra.iter().zip(&rb).enumerate() {
            assert_eq!(x, y, "{pred}: row {i} differs across checkouts");
        }
        for r in &ra {
            if !tiers_seen.contains(&r.1) {
                tiers_seen.push(r.1);
            }
        }
    }
    // Every tier occurs, and negation rows carry their caveats.
    assert_eq!(tiers_seen.len(), 3, "{tiers_seen:?}");
    let unhandled = rendered_rows(&a, "unhandled");
    let caveat = ("handled/1".to_string(), vec!["HANDLED_BY".to_string()], false);
    assert!(unhandled.iter().all(|r| r.2 == [caveat.clone()]), "{:?}", unhandled.first());

    // The comparison really ran against other ids: equal renders, unequal NodeIds.
    let (ta, tb) = (rows(&a, "tc"), rows(&b, "tc"));
    assert_ne!(ta[0].values, tb[0].values, "resalted ids must differ");

    // The warnings and every counter but the clock agree too.
    assert_eq!(a.warnings(), b.warnings());
    let (mut sa, mut sb) = (a.stats().clone(), b.stats().clone());
    (sa.elapsed_ms, sb.elapsed_ms) = (0.0, 0.0);
    assert_eq!(sa, sb);
    assert_eq!(sa.magic_rewrites, 1, "link is rewritten: {sa:?}");
    assert_eq!(sa.aggregates, 4, "{sa:?}");
}

#[test]
fn same_witnesses_across_checkouts() {
    let g = graph();
    let g2 = resalted(&g, 99);
    let (mut a, mut b) = (eval(&g), eval(&g2));
    let limits = ExplainLimits::default();
    let mut ordinals_differ = false;
    for pred in ["tc", "hit", "cyc"] {
        let (ra, rb) = (sample(&rows(&a, pred)), sample(&rows(&b, pred)));
        assert!(ra.len() >= 200, "{pred}: {} rows sampled", ra.len());
        assert_eq!(ra.len(), rb.len());
        let wa = a.explain_all(&ra, &limits).unwrap_or_else(|e| panic!("explain {pred}: {e}"));
        let wb = b.explain_all(&rb, &limits).unwrap_or_else(|e| panic!("explain {pred}: {e}"));
        for (i, (x, y)) in wa.iter().zip(&wb).enumerate() {
            assert_eq!(rendered_witness(&a, x), rendered_witness(&b, y), "{pred}: witness {i}");
            let (ha, hb) = (x.leaf_edges(), y.leaf_edges());
            let hops_a: Vec<String> = ha.iter().map(|h| hop(&a, h)).collect();
            let hops_b: Vec<String> = hb.iter().map(|h| hop(&b, h)).collect();
            assert!(!hops_a.is_empty(), "{pred}: witness {i} reads no edge");
            assert_eq!(hops_a, hops_b, "{pred}: witness {i} hops");
            let (oa, ob): (Vec<usize>, Vec<usize>) =
                (ha.iter().map(|h| h.ordinal).collect(), hb.iter().map(|h| h.ordinal).collect());
            ordinals_differ |= oa != ob;
        }
    }
    // Edge order is reversed, so the stored ordinals moved while the hops did not.
    assert!(ordinals_differ, "no ordinal moved: the resalted edge order was not exercised");
}

#[test]
fn repeat_runs_identical() {
    let g = graph();
    let (mut a, mut b) = (eval(&g), eval(&g));
    let limits = ExplainLimits::default();
    for pred in a.outputs() {
        let (ra, rb) = (rows(&a, &pred), rows(&b, &pred));
        assert_eq!(ra, rb, "{pred}: rows differ between two runs");
        if ["tc/2", "hit/2", "cyc/2", "named/2", "fan_in/2"].contains(&pred.as_str()) {
            let some = sample(&ra);
            let wa = a.explain_all(&some, &limits).unwrap_or_else(|e| panic!("{pred}: {e}"));
            let wb = b.explain_all(&some, &limits).unwrap_or_else(|e| panic!("{pred}: {e}"));
            assert!(wa == wb, "{pred}: witnesses differ between two runs");
        }
    }
    let (mut sa, mut sb) = (a.stats().clone(), b.stats().clone());
    (sa.elapsed_ms, sb.elapsed_ms) = (0.0, 0.0);
    assert_eq!(sa, sb);
}

// ============================================================================
// The source guard
// ============================================================================

/// Every `.rs` file under `dir`, recursively, paths sorted.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display()));
    let mut paths: Vec<PathBuf> = entries
        .map(|e| e.unwrap_or_else(|e| panic!("read_dir {}: {e}", dir.display())).path())
        .collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Per line: is it inside the file's test module (`#[cfg(test)]` at column 0 directly
/// above `mod <name> {`, through the `}` at column 0 that closes it)?
fn test_lines(lines: &[&str]) -> Vec<bool> {
    let mut out = vec![false; lines.len()];
    let mut i = 0;
    while i < lines.len() {
        let opens = lines[i] == "#[cfg(test)]"
            && lines.get(i + 1).is_some_and(|l| l.starts_with("mod ") && l.ends_with('{'));
        if !opens {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < lines.len() {
            out[j] = true;
            if j > i + 1 && lines[j] == "}" {
                break;
            }
            j += 1;
        }
        i = j + 1;
    }
    out
}

/// Lookup-map calls that would expose its order.
const ITERATION: [&str; 6] =
    [".iter()", ".iter_mut()", ".keys()", ".values()", ".drain()", ".into_iter()"];

/// One file's guard result: its offences as `rules/src/<name>:<line>: ...`, and (for
/// `value.rs`) whether Lookup's inner map is a private `HashMap` field.
struct Scan {
    offences: Vec<String>,
    lookup_field: bool,
}

/// [`source_guard`]'s rules over one file, `name` relative to `rules/src`.
fn scan(name: &str, text: &str) -> Scan {
    let lines: Vec<&str> = text.lines().collect();
    let in_test = test_lines(&lines);
    let is_value = name == "value.rs";
    let mut out = Scan { offences: Vec::new(), lookup_field: false };
    for (n, line) in lines.iter().enumerate() {
        let at = format!("rules/src/{name}:{}", n + 1);
        let mut flag = |what: &str| out.offences.push(format!("{at}: {what}: {}", line.trim()));
        for word in ["thread::spawn", "rayon", "par_iter"] {
            if line.contains(word) {
                flag(&format!("`{word}`"));
            }
        }
        if !in_test[n] {
            if line.contains("HashSet") {
                flag("`HashSet`");
            }
            if line.contains("HashMap") && !is_value {
                flag("`HashMap` outside value.rs");
            }
        }
        if !is_value {
            continue;
        }
        let t = line.trim_start();
        if line.contains("self.0") {
            for it in ITERATION {
                if line.contains(it) {
                    flag(&format!("`{it}` on Lookup's map"));
                }
            }
            if t.starts_with("for ")
                || t.contains(" in self.0")
                || t.contains(" in &self.0")
                || t.contains(" in &mut self.0")
            {
                flag("a `for` loop over Lookup's map");
            }
        }
        if t.starts_with("impl") && t.contains("Lookup") && t.contains("Iterator") {
            flag("an iterator impl on Lookup");
        }
        if line.contains("struct Lookup<") {
            out.lookup_field = line.contains("(HashMap<") && !line.contains("(pub");
        }
    }
    out
}

/// The determinism guard over the evaluator's source (plan section 2 DA: no hash map
/// iteration, no threads). Over every `.rs` file under `rules/src`, recursively:
///
/// - no line names `thread::spawn`, `rayon` or `par_iter`;
/// - shipped code (every line outside the file's `#[cfg(test)]` module) names no
///   `HashSet`, and `HashMap` only in `value.rs`, the one sanctioned map (`Lookup`):
///   say `Lookup` in prose. A test module may hold a hash-set oracle (closure.rs's
///   reach oracle does): it is compiled only under test and reaches no output;
/// - in `value.rs`, Lookup's inner map is a private field, no line applies `.iter()`,
///   `.iter_mut()`, `.keys()`, `.values()`, `.drain()`, `.into_iter()` or a `for` loop
///   to it (`self.0`), and no `impl` gives Lookup an iterator.
#[test]
fn source_guard() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    let names: Vec<String> = files
        .iter()
        .map(|p| p.strip_prefix(&root).map_or_else(|_| p.display(), |r| r.display()).to_string())
        .collect();
    for must in ["value.rs", "eval.rs", "exec.rs", "explain.rs", "gql/lower.rs"] {
        assert!(names.iter().any(|n| n == must), "{must} not scanned: {names:?}");
    }
    let mut offences: Vec<String> = Vec::new();
    let mut lookup_field = false;
    for (path, name) in files.iter().zip(&names) {
        let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{name}: {e}"));
        let s = scan(name, &text);
        offences.extend(s.offences);
        lookup_field |= s.lookup_field;
    }
    assert!(lookup_field, "value.rs: Lookup's inner map must be a private HashMap field");
    assert!(offences.is_empty(), "determinism guard:\n{}", offences.join("\n"));
}

/// The guard flags what it says it flags, at file:line, and spares a test module.
#[test]
fn source_guard_catches() {
    let shipped = "use std::collections::HashSet;\nfn f() {\n    let m: HashMap<u32, u32> = HashMap::new();\n    rayon::scope(|_| xs.par_iter());\n}\n";
    let found = scan("gql/new.rs", shipped).offences;
    assert_eq!(found.len(), 4, "{found:?}");
    assert!(found[0].starts_with("rules/src/gql/new.rs:1: `HashSet`"), "{found:?}");
    assert!(found[1].starts_with("rules/src/gql/new.rs:3: `HashMap` outside value.rs"));
    assert!(found[2].starts_with("rules/src/gql/new.rs:4: `rayon`"), "{found:?}");
    assert!(found[3].starts_with("rules/src/gql/new.rs:4: `par_iter`"), "{found:?}");

    // A test module's oracle set is spared; a thread in it is not.
    let tests = "fn g() {}\n\n#[cfg(test)]\nmod tests {\n    use std::collections::HashSet;\n    fn t() { std::thread::spawn(|| ()); }\n}\nstruct After(HashSet<u8>);\n";
    let found = scan("closure.rs", tests).offences;
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].starts_with("rules/src/closure.rs:6: `thread::spawn`"), "{found:?}");
    assert!(found[1].starts_with("rules/src/closure.rs:8: `HashSet`"), "{found:?}");

    // value.rs: the map is allowed, iterating it is not.
    let value = "pub(crate) struct Lookup<K, V>(HashMap<K, V>);\n        self.0.get(k)\n        self.0.keys().next()\n        for (k, v) in &self.0 {\nimpl<K, V> IntoIterator for Lookup<K, V> {\n";
    let s = scan("value.rs", value);
    assert!(s.lookup_field);
    assert_eq!(s.offences.len(), 3, "{:?}", s.offences);
    assert!(s.offences[0].starts_with("rules/src/value.rs:3: `.keys()` on Lookup's map"));
    assert!(s.offences[1].starts_with("rules/src/value.rs:4: a `for` loop over Lookup's map"));
    assert!(s.offences[2].starts_with("rules/src/value.rs:5: an iterator impl on Lookup"));
    assert!(!scan("value.rs", "pub(crate) struct Lookup<K, V>(pub HashMap<K, V>);").lookup_field);
}
