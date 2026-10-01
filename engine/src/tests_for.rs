//! `tests_for` (LE.3b): the tests to run for a change — reverse reachability
//! from the changed nodes to the test cases that exercise them, located and
//! tiered.
//!
//! "Which tests should I run?" is transitive (`price <- place <- audited_place
//! <- test_audited_place`) and crosses boundaries (an integration test calls
//! `GET /users`, whose ROUTE is HANDLED_BY the changed handler). Blast radius
//! backward lists every caller with no test / non-test distinction; this
//! answers with the test cases only.
//!
//! Three entry points share one answer, [`TestsFor`]:
//!
//! - [`tests_for`]: seeds named by qname or bare name — find's exact tiers
//!   (`find::search` + `find::is_exact`, so `shop.orders.price` finds its
//!   `::` qname too); a name no node has goes to `unresolved`;
//! - [`tests_for_diff`]: seeds are the nodes a unified diff's added lines sit
//!   in (`MergedGraph::resolve_signal(diff, "diff")`, the narrowest span per
//!   line; a plain changed-file list seeds every node of each file);
//! - [`tests_for_rev`]: seeds are what the working tree's change against a git
//!   rev did to the graph (LE.1b `delta::graph_delta_vs_rev`): the added,
//!   modified and moved nodes plus the surviving endpoints of every added or
//!   removed edge, walked in the working tree's graph. A deleted test makes the
//!   node it called a seed, so the tests still reaching it are listed.
//!
//! Rev mode's answer is [`tests_for_delta`] of a `RevDelta` the caller already
//! holds; `tests_for_rev` is that call on a fresh `graph_delta_vs_rev`. A
//! caller answering tests AND impact for one change (CC.1) computes the delta
//! once and passes it to both `tests_for_delta` and
//! `diff_impact::diff_impact_from_delta`: one before + after build pair, one
//! `[delta] base=` line, not two.
//!
//! # The walk
//!
//! One [`Adjacency`] over [`TEST_REACH`] (CALLS, USES, INJECTS, IMPLEMENTS,
//! TESTS, HANDLED_BY, HTTP_CALLS, GRPC_CALLS, RPC_CALLS, GRAPHQL_CALLS), then
//! one backward `reach::bfs` per seed, `max_depth` hops. Reverse HANDLED_BY +
//! HTTP_CALLS is how an integration test reaches a handler (`handler <- ROUTE
//! <- ENDPOINT <- test`). Queue and event flows are left out: a test that
//! publishes a message does not run the consumer synchronously. Two kinds of
//! edge are dropped from the index:
//!
//! - every edge INTO a test case, so a case ends the walk on its branch (it
//!   is an answer, not a hop): what calls a test case is never a test of the
//!   seed through it;
//! - the module-level TESTS edges (a test MODULE -> the MODULE its name pairs
//!   with, `passes::emit_tests_edges`): name-convention pairing is the
//!   heuristic tier below and is never walked into a fact or derived row;
//!   so are the function-level TESTS edges `emit_tests_edges` derives from
//!   that pairing and a case's CALLS (CL.5b, EVIDENCE `pass:tests` /
//!   `calls_into_tested_module`): the case reaches its callee over the CALLS
//!   edge itself, so its row stays `derived`, never a `fact` source.
//!
//! # Test cases
//!
//! Read off what the build already stamps, language-agnostically:
//!
//! - a FUNCTION / METHOD with ORIGIN provenance `test_fixture` (a node in a
//!   test file, `passes::tag_synthetic_provenance`) that no other
//!   `test_fixture` node CALLS: helpers are called, cases are roots — so a
//!   JUnit `@Test shouldX()` qualifies by provenance and the root rule, not by
//!   its name;
//! - any FUNCTION / METHOD that is the source of a TESTS edge (pytest
//!   `test_*`, which the Python parser pairs with what it calls), the
//!   derived function-level edges above excepted;
//! - a `test_fixture` MODULE the walk reaches: a Jest / Mocha file's
//!   `describe` / `it` callbacks are anonymous, so the TypeScript parser
//!   attributes their calls to the file's MODULE, and that module is the unit
//!   a runner takes. Only a module-level edge (CALLS, USES, ...) reaches a
//!   module, never a module TESTS pairing (dropped above).
//!
//! A seed that is itself a test case (the change edits a test) is a row of its
//! own: tier `fact`, reason `changed_test`, depth 0.
//!
//! # Tiers
//!
//! - `fact` / `tests_edge`: a TESTS edge from the case straight to the seed
//!   (whatever category first reached it in the walk); depth 1;
//! - `derived` / `reaches`: the walk reached the case through one or more
//!   edges (a direct CALLS from a case with no TESTS edge is derived, depth 1);
//! - `heuristic` / `module_tests_edge` (`module_level`, default on): a MODULE
//!   on the seed's nav parent chain (the seed itself when it is one) is the
//!   target of a module TESTS edge. Reported as the test MODULE itself, never
//!   expanded into its cases, and never promoted: it is name-convention
//!   pairing;
//! - `heuristic` / `cochange` (`signals`, default on): a test MODULE whose
//!   file changes with a seed's file in git history (# Signals below).
//!
//! A test reached from several seeds is one row: `covers` lists every seed it
//! covers (qnames, sorted), `tier` / `reason` / `depth` / `path` are its best
//! hit's, by (tier, depth, seed qname). `path` is the witness from the test to
//! that seed, one `(qname, category)` per hop: the node the hop lands on and
//! the edge category it takes, so `path.len() == depth` and the last entry is
//! the seed. A heuristic row's path is the module TESTS hop, then the nav
//! parent -> child hops down to the seed, labelled DEFINES.
//!
//! # Signals (CC.9a)
//!
//! With `signals` on (the default), one pass over the graph's FAIL cells
//! (LF.6b, read through `external::signals::fail_entries`), module churn ATTN
//! (`module_churn`) and CO_CHANGES edges (LF.5b, `pair_counts`) marks each row
//! with what predicts a failure, in this order:
//!
//! - [`FAILED_LAST_RUN`]: the test node carries a FAIL entry with role `test`
//!   whose newest failure is the newest ingested run's (a MODULE row: a
//!   FUNCTION / METHOD it defines, at any depth, does). The test snapshot
//!   keeps a window of runs (CC.9b): an entry's `last_failed_seq` equals its
//!   `latest_seq` (`FailEntry::failed_latest`; a 0.5.0 entry, from a one-run
//!   snapshot, always does). The row's `fails` / `window` are the test's own
//!   entries' most runs failed in and the runs the snapshot held;
//! - [`SEED_ON_FAILING_TRACE`]: a seed the row covers carries a FAIL entry
//!   with role `implicated` (a frame of a failing test's trace, in any run of
//!   the window);
//! - [`COCHANGE`]: a MODULE on the test's nav parent chain (the node itself
//!   when it is one) and a MODULE on a covered seed's chain are joined by a
//!   CO_CHANGES edge. `cochange_permille` is `1000 * cochanges / commits` of
//!   the seed's module (its churn ATTN): "when the seed's file changed, this
//!   test file changed too", the best over the covered seeds; `None` when no
//!   covered seed's module carries churn.
//!
//! Co-change also adds rows: for each seed's module, every CO_CHANGES
//! neighbour MODULE with ORIGIN provenance `test_fixture` that is no row and
//! holds no row yet becomes one: the MODULE itself (co-change is file-level,
//! like the module TESTS pairing), tier `heuristic`, reason `cochange`, its
//! path the CO_CHANGES hop onto the seed's module, then the nav parent ->
//! child hops down to the seed, labelled DEFINES. Never promoted, and never
//! counted as testing its seed: history is not a code reference.
//!
//! # Order, scope and limit
//!
//! Rows are ordered by (failed_last_run first; fails, highest first and
//! `None` last; seed_on_failing_trace first; tier: fact, derived, heuristic;
//! cochange_permille, highest first and `None` last; depth; file, an
//! unlocated row last; qname), located through one LD.1 `Locator` (1-based
//! lines). With no FAIL / ATTN / CO_CHANGES in the graph, or `signals` off,
//! the signal keys (failed_last_run, fails, seed_on_failing_trace,
//! cochange_permille) are equal on every row and the order is the structural
//! (tier, depth, file, qname). `scope` keeps the rows
//! whose file is under it, with `node_in_scope`'s rules (a path or project
//! label; an unlocatable row is kept). `limit` then keeps the first N rows
//! (`Some(0)` is an error) and `omitted` counts the rest. `test_files` is the
//! distinct files of the kept rows, sorted — what `glia tests-for
//! --files-only` hands a test runner. `untested` is the seeds no test case
//! reaches: a heuristic module pairing or a co-change alone does not count,
//! and it is computed before `scope` and `limit` (a seed tested only outside
//! the scope is still tested). `absence` (LD.8a) is `Some` exactly when
//! `tests` is empty.
//!
//! Seeds are deduplicated and ordered by (qname, id). More than
//! [`MAX_SEEDS`] is an error, never a silent cut.
//!
//! fired_on markers, per answer: always
//! `[tests-for] seeds=<S> tests=<T> fact=<F> derived=<D> heuristic=<H> untested=<U> files=<N>`
//! — grep `^\[tests-for\] seeds=` — then, when the signal pass ran,
//! `[tests-for] signals failed_last_run=<A> on_failing_trace=<B> cochange=<C> cochange_only=<D> omitted=<E>`
//! — grep `^\[tests-for\] signals `. Both count the rows returned (after
//! `scope` and `limit`); `omitted` is what `limit` cut.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use glia_activation::algo::reach;
use glia_activation::algo::{Adjacency, CategorySet, GraphSource, Walk};
use glia_code_domain::evidence::Evidence;
use glia_code_domain::{cell_type, edge_category, node_kind};
use glia_core::{CellPayload, Edge, EdgeCategoryId, NodeId, NodeKindId};
use glia_graph::MergedGraph;

use crate::absence::{self, Absence};
use crate::answers::{Locator, in_scope, resolve_scope};
use crate::external::signals::{self, FailRole};
use crate::find::{self, FindOptions, FoundNode};
use crate::passes::{FN_TESTS_EMITTER, FN_TESTS_RULE};

/// [`TestsForArgs::default`]'s `max_depth`.
pub const DEFAULT_MAX_DEPTH: usize = 6;

/// The most seeds one answer walks from; more is an error.
pub const MAX_SEEDS: usize = 256;

/// The edge categories the backward walk follows (module docs).
pub const TEST_REACH: [EdgeCategoryId; 10] = [
    edge_category::CALLS,
    edge_category::USES,
    edge_category::INJECTS,
    edge_category::IMPLEMENTS,
    edge_category::TESTS,
    edge_category::HANDLED_BY,
    edge_category::HTTP_CALLS,
    edge_category::GRPC_CALLS,
    edge_category::RPC_CALLS,
    edge_category::GRAPHQL_CALLS,
];

/// The mechanisms an empty answer's absence names, with their caveat rows.
const MECHANISMS: &[&str] = &["TESTS", "CALLS", "HTTP_CALLS"];

/// The primitive name in the `[absence]` marker.
const PRIMITIVE: &str = "tests_for";

pub const FACT: &str = "fact";
pub const DERIVED: &str = "derived";
pub const HEURISTIC: &str = "heuristic";

/// A row signal (module docs): the test failed in the newest ingested run.
pub const FAILED_LAST_RUN: &str = "failed_last_run";
/// A row signal: a seed it covers is a frame of a failing test's trace.
pub const SEED_ON_FAILING_TRACE: &str = "seed_on_failing_trace";
/// A row signal: its file changes with a covered seed's file in git history.
pub const COCHANGE: &str = "cochange";

/// The reason of a co-change-only row.
const COCHANGE_REASON: &str = "cochange";

/// How far and how wide [`tests_for`] looks. Start from `default()` and set
/// fields: `#[non_exhaustive]` rules out a struct literal outside this crate.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct TestsForArgs {
    /// Hops of the backward walk from each seed ([`DEFAULT_MAX_DEPTH`]).
    pub max_depth: usize,
    /// Keep only rows whose file is under this path or project label.
    pub scope: Option<String>,
    /// Add the heuristic tier: the test MODULEs a module TESTS edge pairs with
    /// a seed's module (default `true`).
    pub module_level: bool,
    /// Keep the first N rows after ranking and `scope` (`None`: every row;
    /// `Some(0)` is an error). The rest are counted in [`TestsFor::omitted`].
    pub limit: Option<usize>,
    /// Read the FAIL / churn ATTN / CO_CHANGES signals, rank by them and add
    /// the co-change-only rows (default `true`; module docs).
    pub signals: bool,
}

impl Default for TestsForArgs {
    fn default() -> Self {
        TestsForArgs {
            max_depth: DEFAULT_MAX_DEPTH,
            scope: None,
            module_level: true,
            limit: None,
            signals: true,
        }
    }
}

/// One test to run (module docs): identity, 1-based location, tier and
/// reason, the depth of its best hit, the seeds it covers and the witness
/// path from it to its best-hit seed.
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct TestHit {
    pub qname: String,
    pub name: String,
    pub kind: &'static str,
    pub file: Option<String>,
    /// 1-based.
    pub line: Option<i64>,
    /// `fact` | `derived` | `heuristic`.
    pub tier: &'static str,
    /// `tests_edge` | `reaches` | `module_tests_edge` | `changed_test` |
    /// `cochange`.
    pub reason: &'static str,
    pub depth: usize,
    /// The seeds this test covers, qnames sorted.
    pub covers: Vec<String>,
    /// `(qname, category)` per hop, from the test to its best-hit seed.
    pub path: Vec<(String, &'static str)>,
    /// The signals the row carries, in the order [`FAILED_LAST_RUN`],
    /// [`SEED_ON_FAILING_TRACE`], [`COCHANGE`]; empty with `signals` off.
    pub signals: Vec<&'static str>,
    /// With [`COCHANGE`]: per mille of the covered seed's module commits
    /// that also changed this test's file, the best over covered seeds.
    pub cochange_permille: Option<u32>,
    /// With a FAIL entry of role `test` on the row's own node (CC.9b): the
    /// most runs of the test snapshot's window one of its failures happened
    /// in. `None` with no failure ingested for it, or `signals` off.
    pub fails: Option<u32>,
    /// With `fails`: the runs the test snapshot held.
    pub window: Option<u32>,
}

/// The answer (module docs).
#[non_exhaustive]
#[derive(serde::Serialize, Debug, Clone)]
pub struct TestsFor {
    /// The seed qnames, ordered by (qname, id).
    pub seeds: Vec<String>,
    pub tests: Vec<TestHit>,
    /// Rows ranked and in scope that `limit` cut.
    pub omitted: usize,
    /// The distinct files of `tests`, sorted.
    pub test_files: Vec<String>,
    /// Seeds no test case reaches (heuristic pairings do not count).
    pub untested: Vec<String>,
    /// Names given to [`tests_for`] that no node has.
    pub unresolved: Vec<String>,
    /// `Some` iff `tests` is empty: why.
    pub absence: Option<Absence>,
}

/// The tests for the nodes `qnames` name (a qname or a bare name each; module
/// docs). `Err` when `qnames` is empty or resolves to more than
/// [`MAX_SEEDS`] nodes.
pub fn tests_for(
    merged: &MergedGraph,
    qnames: &[&str],
    args: &TestsForArgs,
) -> Result<TestsFor, String> {
    if qnames.iter().all(|q| q.trim().is_empty()) {
        return Err("tests_for: no seed qname given".to_string());
    }
    let opts = FindOptions {
        top_k: absence::SUGGESTIONS,
        ..FindOptions::default()
    };
    let mut ids: Vec<NodeId> = Vec::new();
    let mut unresolved: Vec<String> = Vec::new();
    let mut near: Option<Vec<FoundNode>> = None;
    for q in qnames.iter().map(|q| q.trim()).filter(|q| !q.is_empty()) {
        let rows = find::search(merged, q, &opts).rows;
        match rows.first().filter(|r| find::is_exact(r)) {
            Some(r) => ids.push(NodeId(r.id)),
            None => {
                unresolved.push(q.to_string());
                near.get_or_insert(rows);
            }
        }
    }
    let query = qnames.join(", ");
    let no_seed = |merged: &MergedGraph| {
        let first = unresolved
            .first()
            .map(String::as_str)
            .unwrap_or(query.as_str());
        absence::unknown_symbol(
            merged,
            PRIMITIVE,
            first,
            MECHANISMS,
            near.as_deref().unwrap_or(&[]),
        )
    };
    let mut answer = answer_for(merged, &ids, &query, args, no_seed)?;
    answer.unresolved = unresolved;
    Ok(answer)
}

/// The tests for the nodes a unified diff (or a changed-file list) touches
/// (module docs). `Err` beyond [`MAX_SEEDS`] seeds.
pub fn tests_for_diff(
    merged: &MergedGraph,
    diff_text: &str,
    args: &TestsForArgs,
) -> Result<TestsFor, String> {
    let ids = merged.resolve_signal(diff_text, "diff");
    let files: Vec<&str> = diff_text
        .lines()
        .filter_map(|l| l.strip_prefix("+++ "))
        .map(|p| p.split('\t').next().unwrap_or(p).trim())
        .map(|p| p.strip_prefix("b/").unwrap_or(p))
        .filter(|p| *p != "/dev/null")
        .collect();
    let query = if files.is_empty() {
        "diff".to_string()
    } else {
        format!("diff: {}", files.join(", "))
    };
    answer_for(merged, &ids, &query, args, |merged| {
        let note = "no added line of the diff sits in a node of this graph".to_string();
        absence::empty(
            merged,
            PRIMITIVE,
            &query,
            "no_signal_match",
            note,
            MECHANISMS,
            None,
        )
    })
}

/// The tests for what the working tree's change against git rev `base` did to
/// the graph (module docs), walked in the working tree's graph. `Err` on
/// `delta::graph_delta_vs_rev`'s errors (not a directory, not a git work
/// tree, an unknown rev, a failed build) and beyond [`MAX_SEEDS`] seeds.
pub fn tests_for_rev(repo_path: &str, base: &str, args: &TestsForArgs) -> Result<TestsFor, String> {
    let rev = crate::delta::graph_delta_vs_rev(repo_path, base)?;
    tests_for_delta(&rev, args)
}

/// The tests for an already computed rev delta (module docs), walked in
/// `rev.after`'s graph. Nothing is built; the absence query is
/// `rev <rev.answer.base>`, the rev as `graph_delta_vs_rev` was given it.
/// `Err` beyond [`MAX_SEEDS`] seeds only. `rev` holds both sides' graphs, so
/// a caller that keeps it alive to ask more questions
/// (`diff_impact::diff_impact_from_delta`) holds two graphs in memory.
pub fn tests_for_delta(
    rev: &crate::delta::RevDelta,
    args: &TestsForArgs,
) -> Result<TestsFor, String> {
    let ids = rev_seeds(rev);
    let merged = &rev.after.merged;
    let base = &rev.answer.base;
    let query = format!("rev {base}");
    answer_for(merged, &ids, &query, args, |merged| {
        let note = format!(
            "the working tree's change against {base} changes no node, and no edge the walk follows, in this graph"
        );
        absence::empty(
            merged, PRIMITIVE, &query, "no_match", note, MECHANISMS, None,
        )
    })
}

/// The rev mode's seeds, as after-graph ids: the added and moved nodes, the
/// modified nodes whose OWN text changed, and the surviving endpoints of every
/// added or removed edge of a walked category ([`TEST_REACH`], minus the
/// module-level TESTS pairings and the function-level TESTS edges derived
/// from them, [`is_derived_fn_tests`]).
///
/// A container's CODE cell is its whole span, so LE.1a marks a module or a
/// class modified whenever a line inside one of its functions changes. Such a
/// node is a seed only when its text with every direct child's text cut out
/// differs between the two sides (a module-level import beside the edited
/// function): an edit inside `price` seeds `price`, not also its module —
/// what diff mode, which takes each changed line's narrowest node, seeds too.
/// A structural edge (DEFINES, IMPORTS, ...) gained or lost says nothing a
/// test walk follows, so its endpoints are not seeds.
fn rev_seeds(rev: &crate::delta::RevDelta) -> Vec<NodeId> {
    let (before, after) = (&rev.before.merged, &rev.after.merged);
    let d = &rev.delta;
    let (old, new) = (CodeText::new(before), CodeText::new(after));
    let present: HashSet<NodeId> = after
        .graphs
        .iter()
        .flat_map(|g| g.nav.qname_by_id.keys().copied())
        .collect();
    let moved_to: HashMap<NodeId, NodeId> = d.moved_nodes.iter().map(|&(b, a)| (b, a)).collect();
    let was: HashMap<NodeId, NodeId> = d.moved_nodes.iter().map(|&(b, a)| (a, b)).collect();
    let (old_fn_tests, new_fn_tests) = (derived_fn_tests(before), derived_fn_tests(after));
    let walked = |c: &CodeText<'_>,
                  fn_tests: &HashSet<(NodeId, NodeId)>,
                  k: &glia_activation::algo::delta::EdgeKey| {
        TEST_REACH.contains(&k.category)
            && !(k.category == edge_category::TESTS
                && (c.kind.get(&k.from) == Some(&node_kind::MODULE)
                    || fn_tests.contains(&(k.from, k.to))))
    };
    let mut ids: Vec<NodeId> = Vec::new();
    ids.extend(d.added_nodes.iter().copied());
    ids.extend(d.moved_nodes.iter().map(|&(_, a)| a));
    for &id in &d.modified_nodes {
        let prior = was.get(&id).copied().unwrap_or(id);
        if !new.is_container(id) || new.own_text(id) != old.own_text(prior) {
            ids.push(id);
        }
    }
    for k in d.added_edges.iter().filter(|k| walked(&new, &new_fn_tests, k)) {
        ids.extend([k.from, k.to]);
    }
    for k in d.removed_edges.iter().filter(|k| walked(&old, &old_fn_tests, k)) {
        for end in [k.from, k.to] {
            ids.push(moved_to.get(&end).copied().unwrap_or(end));
        }
    }
    ids.retain(|id| present.contains(id));
    ids
}

/// The `(from, to)` of every derived function-level TESTS edge of `m`
/// ([`is_derived_fn_tests`]).
fn derived_fn_tests(m: &MergedGraph) -> HashSet<(NodeId, NodeId)> {
    m.all_edges()
        .filter(|e| is_derived_fn_tests(e))
        .map(|e| (e.from, e.to))
        .collect()
}

/// A function-level TESTS edge `passes::emit_tests_edges` derived from a
/// module pairing and a CALLS edge (CL.5b): its EVIDENCE names
/// [`FN_TESTS_EMITTER`] and [`FN_TESTS_RULE`]. Like a module pairing, it is
/// no fact source and no hop of the walk (module docs).
fn is_derived_fn_tests(e: &Edge) -> bool {
    e.category == edge_category::TESTS
        && Evidence::of(e).is_some_and(|ev| {
            ev.emitter == FN_TESTS_EMITTER && ev.rule.as_deref() == Some(FN_TESTS_RULE)
        })
}

/// One graph's node CODE texts, kinds and nav children (the first graph that
/// names a node gives its kind), for [`rev_seeds`]' own-text rule. Shared
/// with LE.2's `diff_impact`, whose rev mode seeds from the same delta.
pub(crate) struct CodeText<'a> {
    code: HashMap<NodeId, Vec<&'a str>>,
    kind: HashMap<NodeId, NodeKindId>,
    children: HashMap<NodeId, BTreeSet<u64>>,
}

impl<'a> CodeText<'a> {
    pub(crate) fn new(m: &'a MergedGraph) -> Self {
        let mut code: HashMap<NodeId, Vec<&'a str>> = HashMap::new();
        let mut kind: HashMap<NodeId, NodeKindId> = HashMap::new();
        let mut children: HashMap<NodeId, BTreeSet<u64>> = HashMap::new();
        for g in &m.graphs {
            for n in &g.nodes {
                for c in &n.cells {
                    if c.kind == cell_type::CODE
                        && let CellPayload::Text(t) = &c.payload
                    {
                        code.entry(n.id).or_default().push(t.as_str());
                    }
                }
                if let Some(&k) = g.nav.kind_by_id.get(&n.id) {
                    kind.entry(n.id).or_insert(k);
                }
            }
            for (p, kids) in &g.nav.children_of {
                children
                    .entry(*p)
                    .or_default()
                    .extend(kids.iter().map(|k| k.0));
            }
        }
        CodeText {
            code,
            kind,
            children,
        }
    }

    pub(crate) fn is_container(&self, id: NodeId) -> bool {
        self.children.get(&id).is_some_and(|k| !k.is_empty())
    }

    /// `id`'s CODE texts with every direct child's CODE text cut out and all
    /// whitespace dropped (the blank lines around an added function are not a
    /// change of its module), sorted and deduplicated. Children are cut
    /// longest first (ties by text), so the result never depends on map order.
    pub(crate) fn own_text(&self, id: NodeId) -> Vec<String> {
        let mut kids: Vec<&str> = self
            .children
            .get(&id)
            .into_iter()
            .flatten()
            .flat_map(|k| self.code.get(&NodeId(*k)).into_iter().flatten().copied())
            .filter(|t| !t.is_empty())
            .collect();
        kids.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        kids.dedup();
        let mut out: Vec<String> = self
            .code
            .get(&id)
            .into_iter()
            .flatten()
            .map(|t| {
                let own = kids.iter().fold(t.to_string(), |acc, k| acc.replace(k, ""));
                own.split_whitespace().collect::<String>()
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// One hit of a test on a seed, before rows merge.
struct Hit {
    rank: u8,
    depth: usize,
    reason: &'static str,
    path: Vec<(NodeId, EdgeCategoryId)>,
}

/// A row being merged: its best hit, keyed with the seed qname, and every
/// seed it covers (qnames for the answer, ids for the signal pass).
struct Row {
    best: (u8, usize, String),
    hit: Hit,
    covers: BTreeSet<String>,
    seed_ids: BTreeSet<u64>,
}

/// Merge `hit` of `test` on the seed `(qname, id)` into `rows`: the seed is
/// covered, and the hit replaces the row's best when its (tier, depth, seed
/// qname) is lower.
fn add_hit(rows: &mut BTreeMap<u64, Row>, test: NodeId, seed: (&str, NodeId), hit: Hit) {
    let key = (hit.rank, hit.depth, seed.0.to_string());
    let row = rows.entry(test.0).or_insert_with(|| Row {
        best: (u8::MAX, usize::MAX, String::new()),
        hit: Hit {
            rank: u8::MAX,
            depth: 0,
            reason: "",
            path: Vec::new(),
        },
        covers: BTreeSet::new(),
        seed_ids: BTreeSet::new(),
    });
    row.covers.insert(seed.0.to_string());
    row.seed_ids.insert(seed.1.0);
    if key < row.best {
        row.best = key;
        row.hit = hit;
    }
}

/// The answer over resolved seed ids; `no_seed` builds the absence when
/// there are none.
fn answer_for(
    merged: &MergedGraph,
    ids: &[NodeId],
    query: &str,
    args: &TestsForArgs,
    no_seed: impl FnOnce(&MergedGraph) -> Absence,
) -> Result<TestsFor, String> {
    if args.limit == Some(0) {
        return Err("tests_for: a limit of 0 keeps no test; give 1 or more".to_string());
    }
    let loc = Locator::new(merged);
    let mut seeds: Vec<(String, NodeId)> = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    for &id in ids {
        if seen.insert(id) {
            seeds.push((loc.locate(id).qname, id));
        }
    }
    if seeds.len() > MAX_SEEDS {
        return Err(format!(
            "tests_for: {} seeds, more than the {MAX_SEEDS} one answer walks from; narrow the change",
            seeds.len()
        ));
    }
    seeds.sort_by(|a, b| (&a.0, a.1.0).cmp(&(&b.0, b.1.0)));

    let idx = TestIndex::build(merged);
    let source = ReachSource { merged, idx: &idx };
    let adj = Adjacency::build(&source, &CategorySet::of(&TEST_REACH));

    let mut rows: BTreeMap<u64, Row> = BTreeMap::new();
    let mut untested: Vec<String> = Vec::new();
    for (qname, seed) in &seeds {
        let seed_key = (qname.as_str(), *seed);
        let mut tested = false;
        if idx.is_case(*seed) {
            add_hit(
                &mut rows,
                *seed,
                seed_key,
                Hit {
                    rank: 0,
                    depth: 0,
                    reason: "changed_test",
                    path: Vec::new(),
                },
            );
            tested = true;
        }
        for &test in idx.tests_into.get(seed).map(Vec::as_slice).unwrap_or(&[]) {
            if idx.is_case(test) {
                let path = vec![(*seed, edge_category::TESTS)];
                add_hit(
                    &mut rows,
                    test,
                    seed_key,
                    Hit {
                        rank: 0,
                        depth: 1,
                        reason: "tests_edge",
                        path,
                    },
                );
                tested = true;
            }
        }
        let walk = reach::bfs(&adj, &[*seed], Walk::Backward, args.max_depth);
        let parent: HashMap<NodeId, (NodeId, EdgeCategoryId)> = walk
            .reached
            .iter()
            .map(|r| (r.id, (r.parent, r.via)))
            .collect();
        for r in &walk.reached {
            if !idx.is_case(r.id) {
                continue;
            }
            // The walk runs backward, so each hop from the test toward the
            // seed is the node it was reached from, over the reaching edge.
            let mut path = Vec::with_capacity(r.depth);
            let mut at = r.id;
            while at != *seed {
                let Some(&(next, via)) = parent.get(&at) else {
                    break;
                };
                path.push((next, via));
                at = next;
            }
            add_hit(
                &mut rows,
                r.id,
                seed_key,
                Hit {
                    rank: 1,
                    depth: r.depth,
                    reason: "reaches",
                    path,
                },
            );
            tested = true;
        }
        if args.module_level {
            for (module, below) in idx.module_chain(*seed) {
                for &test in idx
                    .module_tests
                    .get(&module)
                    .map(Vec::as_slice)
                    .unwrap_or(&[])
                {
                    let mut path = vec![(module, edge_category::TESTS)];
                    path.extend(below.iter().rev().map(|&n| (n, edge_category::DEFINES)));
                    let depth = path.len();
                    add_hit(
                        &mut rows,
                        test,
                        seed_key,
                        Hit {
                            rank: 2,
                            depth,
                            reason: "module_tests_edge",
                            path,
                        },
                    );
                }
            }
        }
        if !tested {
            untested.push(qname.clone());
        }
    }

    let sig = args.signals.then(|| Signals::build(merged, &idx));
    if let Some(sig) = &sig {
        sig.add_cochange_rows(&idx, &seeds, &mut rows);
    }

    let mut tests: Vec<TestHit> = rows
        .into_iter()
        .map(|(id, row)| {
            let test = NodeId(id);
            let (signals, cochange_permille) = match &sig {
                Some(sig) => sig.of_row(&idx, test, &row.seed_ids),
                None => (Vec::new(), None),
            };
            let (fails, window) = sig
                .as_ref()
                .and_then(|sig| sig.fails.get(&test))
                .map_or((None, None), |&(f, w)| (Some(f), Some(w)));
            let at = loc.locate(test);
            let path = row
                .hit
                .path
                .iter()
                .map(|&(n, c)| (loc.locate(n).qname, edge_category::name(c)))
                .collect();
            TestHit {
                qname: at.qname,
                name: at.name,
                kind: at.kind,
                file: at.file,
                line: at.line,
                tier: tier_name(row.hit.rank),
                reason: row.hit.reason,
                depth: row.hit.depth,
                covers: row.covers.into_iter().collect(),
                path,
                signals,
                cochange_permille,
                fails,
                window,
            }
        })
        .collect();
    let found = tests.len();
    if let Some(raw) = args.scope.as_deref() {
        let scope = resolve_scope(merged, raw);
        tests.retain(|t| t.file.as_deref().is_none_or(|f| in_scope(f, &scope)));
    }
    tests.sort_by(row_order);
    let omitted = match args.limit {
        Some(n) if tests.len() > n => {
            let cut = tests.len() - n;
            tests.truncate(n);
            cut
        }
        _ => 0,
    };
    let test_files: Vec<String> = tests
        .iter()
        .filter_map(|t| t.file.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let count = |tier: &str| tests.iter().filter(|t| t.tier == tier).count();
    eprintln!(
        "[tests-for] seeds={} tests={} fact={} derived={} heuristic={} untested={} files={}",
        seeds.len(),
        tests.len(),
        count(FACT),
        count(DERIVED),
        count(HEURISTIC),
        untested.len(),
        test_files.len()
    );
    if sig.is_some() {
        let with = |signal: &str| tests.iter().filter(|t| t.signals.contains(&signal)).count();
        eprintln!(
            "[tests-for] signals failed_last_run={} on_failing_trace={} cochange={} cochange_only={} omitted={omitted}",
            with(FAILED_LAST_RUN),
            with(SEED_ON_FAILING_TRACE),
            with(COCHANGE),
            tests.iter().filter(|t| t.reason == COCHANGE_REASON).count(),
        );
    }
    let absence = if !tests.is_empty() {
        None
    } else if seeds.is_empty() {
        Some(no_seed(merged))
    } else if found > 0
        && let Some(raw) = args.scope.as_deref()
    {
        Some(absence::scope_emptied(merged, PRIMITIVE, query, found, raw))
    } else {
        let names: Vec<String> = seeds.iter().map(|(q, _)| format!("`{q}`")).collect();
        let pairing = match (args.module_level, seeds.len()) {
            (false, _) => ", and the module-level pairing was not asked for",
            (true, 1) => ", and no test module is paired with its module",
            (true, _) => ", and no test module is paired with their modules",
        };
        let note = format!(
            "no test case reaches {} over TESTS / CALLS / HTTP_CALLS (and the other test-reach edges) within {} {}{pairing}",
            names.join(", "),
            args.max_depth,
            absence::plural(args.max_depth, "hop", "hops"),
        );
        let seed_file = seeds.iter().find_map(|(_, id)| loc.locate(*id).file);
        Some(absence::empty(
            merged,
            PRIMITIVE,
            query,
            "no_edges",
            note,
            MECHANISMS,
            seed_file.as_deref(),
        ))
    };
    Ok(TestsFor {
        seeds: seeds.into_iter().map(|(q, _)| q).collect(),
        tests,
        omitted,
        test_files,
        untested,
        unresolved: Vec::new(),
        absence,
    })
}

/// The row order (module docs): failed in the last run, then the runs of the
/// window it failed in (most first, `None` last), then a covered seed on a
/// failing trace, then tier, co-change confidence (highest first, `None`
/// last), depth, file (an unlocated row last) and qname. Total: it ends on
/// the qname, and one row per node.
fn row_order(a: &TestHit, b: &TestHit) -> std::cmp::Ordering {
    let lacks = |t: &TestHit, signal: &str| !t.signals.contains(&signal);
    lacks(a, FAILED_LAST_RUN)
        .cmp(&lacks(b, FAILED_LAST_RUN))
        .then_with(|| b.fails.cmp(&a.fails))
        .then_with(|| lacks(a, SEED_ON_FAILING_TRACE).cmp(&lacks(b, SEED_ON_FAILING_TRACE)))
        .then_with(|| tier_rank(a.tier).cmp(&tier_rank(b.tier)))
        .then_with(|| b.cochange_permille.cmp(&a.cochange_permille))
        .then_with(|| {
            (a.depth, a.file.is_none(), &a.file, &a.qname).cmp(&(
                b.depth,
                b.file.is_none(),
                &b.file,
                &b.qname,
            ))
        })
}

/// What the signal pass reads (module docs), in one pass over the nodes and
/// one over the edges. Every map is keyed by id, so no output depends on
/// hash order.
struct Signals {
    /// Nodes carrying a FAIL entry with role `test` that failed in the
    /// newest ingested run.
    failed: HashSet<NodeId>,
    /// Nodes carrying a FAIL entry with role `test` -> the most `fails` over
    /// those entries and that entry's `window` (the larger window on a tie).
    fails: HashMap<NodeId, (u32, u32)>,
    /// The MODULEs on a failed node's nav parent chain: a MODULE row holding
    /// a failing test.
    failing_modules: HashSet<NodeId>,
    /// Nodes carrying a FAIL entry with role `implicated`.
    implicated: HashSet<NodeId>,
    /// MODULE -> each CO_CHANGES neighbour MODULE -> the pair's co-changes
    /// (either edge direction; the most over parallel edges).
    cochange: HashMap<NodeId, BTreeMap<u64, u32>>,
    /// MODULE -> the commits of its churn ATTN (the first graph's copy).
    commits: HashMap<NodeId, u32>,
}

impl Signals {
    fn build(merged: &MergedGraph, idx: &TestIndex) -> Self {
        let mut failed: HashSet<NodeId> = HashSet::new();
        let mut fails: HashMap<NodeId, (u32, u32)> = HashMap::new();
        let mut implicated: HashSet<NodeId> = HashSet::new();
        let mut commits: HashMap<NodeId, u32> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                for e in signals::fail_entries(&n.cells) {
                    match e.role {
                        FailRole::Test => {
                            if e.failed_latest() {
                                failed.insert(n.id);
                            }
                            let most = fails.entry(n.id).or_insert((0, 0));
                            *most = (*most).max((e.fails, e.window));
                        }
                        FailRole::Implicated => {
                            implicated.insert(n.id);
                        }
                    }
                }
                if idx.kind.get(&n.id) == Some(&node_kind::MODULE)
                    && let Some(c) = signals::module_churn(&n.cells)
                {
                    commits.entry(n.id).or_insert(c.commits);
                }
            }
        }
        let failing_modules: HashSet<NodeId> = failed
            .iter()
            .flat_map(|&f| idx.module_chain(f))
            .map(|(m, _)| m)
            .collect();
        let mut cochange: HashMap<NodeId, BTreeMap<u64, u32>> = HashMap::new();
        for e in merged.all_edges() {
            if e.category != edge_category::CO_CHANGES || e.from == e.to {
                continue;
            }
            let Some(p) = signals::pair_counts(&e.cells) else {
                continue;
            };
            for (a, b) in [(e.from, e.to), (e.to, e.from)] {
                let n = cochange.entry(a).or_default().entry(b.0).or_insert(0);
                *n = (*n).max(p.cochanges);
            }
        }
        Signals {
            failed,
            fails,
            failing_modules,
            implicated,
            cochange,
            commits,
        }
    }

    /// Add a `cochange` row for every test MODULE a seed's module co-changes
    /// with that no structural row is, or sits in (module docs).
    fn add_cochange_rows(
        &self,
        idx: &TestIndex,
        seeds: &[(String, NodeId)],
        rows: &mut BTreeMap<u64, Row>,
    ) {
        let held: HashSet<NodeId> = rows
            .keys()
            .flat_map(|&id| idx.module_chain(NodeId(id)))
            .map(|(m, _)| m)
            .collect();
        for (qname, seed) in seeds {
            for (module, below) in idx.module_chain(*seed) {
                for &nb in self.cochange.get(&module).into_iter().flat_map(BTreeMap::keys) {
                    let nb = NodeId(nb);
                    if idx.kind.get(&nb) != Some(&node_kind::MODULE)
                        || !idx.fixture.contains(&nb)
                        || held.contains(&nb)
                    {
                        continue;
                    }
                    let mut path = vec![(module, edge_category::CO_CHANGES)];
                    path.extend(below.iter().rev().map(|&n| (n, edge_category::DEFINES)));
                    let depth = path.len();
                    add_hit(
                        rows,
                        nb,
                        (qname.as_str(), *seed),
                        Hit {
                            rank: 2,
                            depth,
                            reason: COCHANGE_REASON,
                            path,
                        },
                    );
                }
            }
        }
    }

    /// The signals of the row for `test`, covering `seed_ids`, and its
    /// co-change confidence.
    fn of_row(
        &self,
        idx: &TestIndex,
        test: NodeId,
        seed_ids: &BTreeSet<u64>,
    ) -> (Vec<&'static str>, Option<u32>) {
        let mut out = Vec::new();
        let is_module = idx.kind.get(&test) == Some(&node_kind::MODULE);
        if self.failed.contains(&test) || (is_module && self.failing_modules.contains(&test)) {
            out.push(FAILED_LAST_RUN);
        }
        if seed_ids.iter().any(|s| self.implicated.contains(&NodeId(*s))) {
            out.push(SEED_ON_FAILING_TRACE);
        }
        let test_modules: Vec<NodeId> = idx.module_chain(test).into_iter().map(|(m, _)| m).collect();
        let mut cochange = false;
        let mut best: Option<u32> = None;
        for &seed in seed_ids {
            for (module, _) in idx.module_chain(NodeId(seed)) {
                let Some(pairs) = self.cochange.get(&module) else {
                    continue;
                };
                for tm in &test_modules {
                    let Some(&n) = pairs.get(&tm.0) else {
                        continue;
                    };
                    cochange = true;
                    let permille = self
                        .commits
                        .get(&module)
                        .filter(|&&c| c > 0)
                        .map(|&c| (u64::from(n) * 1000 / u64::from(c)).min(1000));
                    if let Some(p) = permille.and_then(|p| u32::try_from(p).ok()) {
                        best = Some(best.map_or(p, |b| b.max(p)));
                    }
                }
            }
        }
        if cochange {
            out.push(COCHANGE);
        }
        (out, best)
    }
}

fn tier_name(rank: u8) -> &'static str {
    match rank {
        0 => FACT,
        1 => DERIVED,
        _ => HEURISTIC,
    }
}

fn tier_rank(tier: &str) -> u8 {
    match tier {
        FACT => 0,
        DERIVED => 1,
        _ => 2,
    }
}

/// What the test-case predicate and the heuristic tier read, built once per
/// answer in O(V + E). Kind and parent are the first graph's (in
/// `merged.graphs` order) that names the node, as `Locator` reads them.
struct TestIndex {
    kind: HashMap<NodeId, NodeKindId>,
    parent: HashMap<NodeId, NodeId>,
    /// Nodes with ORIGIN provenance `test_fixture`, any kind.
    fixture: HashSet<NodeId>,
    /// FUNCTION / METHOD nodes another `test_fixture` node CALLS.
    called_by_test: HashSet<NodeId>,
    /// FUNCTION / METHOD sources of a TESTS edge (the derived function-level
    /// edges excepted, [`is_derived_fn_tests`]).
    tests_sources: HashSet<NodeId>,
    /// Target -> the FUNCTION / METHOD sources of its TESTS edges, in edge
    /// order, deduplicated.
    tests_into: HashMap<NodeId, Vec<NodeId>>,
    /// Target MODULE -> the test MODULEs its module TESTS edges come from, in
    /// edge order, deduplicated.
    module_tests: HashMap<NodeId, Vec<NodeId>>,
}

impl TestIndex {
    fn build(merged: &MergedGraph) -> Self {
        let mut kind: HashMap<NodeId, NodeKindId> = HashMap::new();
        let mut parent: HashMap<NodeId, NodeId> = HashMap::new();
        let mut fixture: HashSet<NodeId> = HashSet::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if let Some(&k) = g.nav.kind_by_id.get(&n.id) {
                    kind.entry(n.id).or_insert(k);
                }
                if let Some(&p) = g.nav.parent_of.get(&n.id) {
                    parent.entry(n.id).or_insert(p);
                }
                if is_test_fixture(&n.cells) {
                    fixture.insert(n.id);
                }
            }
        }
        let callable = |id: &NodeId| {
            matches!(
                kind.get(id),
                Some(&node_kind::FUNCTION | &node_kind::METHOD)
            )
        };
        let module = |id: &NodeId| kind.get(id) == Some(&node_kind::MODULE);
        let mut called_by_test = HashSet::new();
        let mut tests_sources = HashSet::new();
        let mut tests_into: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        let mut module_tests: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for e in merged.all_edges() {
            if e.from == e.to {
                continue;
            }
            if e.category == edge_category::CALLS && fixture.contains(&e.from) && callable(&e.to) {
                called_by_test.insert(e.to);
            }
            if e.category != edge_category::TESTS || is_derived_fn_tests(e) {
                continue;
            }
            if callable(&e.from) {
                tests_sources.insert(e.from);
                let v = tests_into.entry(e.to).or_default();
                if !v.contains(&e.from) {
                    v.push(e.from);
                }
            } else if module(&e.from) && module(&e.to) {
                let v = module_tests.entry(e.to).or_default();
                if !v.contains(&e.from) {
                    v.push(e.from);
                }
            }
        }
        TestIndex {
            kind,
            parent,
            fixture,
            called_by_test,
            tests_sources,
            tests_into,
            module_tests,
        }
    }

    /// Is `id` a test case (module docs)?
    fn is_case(&self, id: NodeId) -> bool {
        match self.kind.get(&id) {
            Some(&node_kind::FUNCTION | &node_kind::METHOD) => {
                self.tests_sources.contains(&id)
                    || (self.fixture.contains(&id) && !self.called_by_test.contains(&id))
            }
            Some(&node_kind::MODULE) => self.fixture.contains(&id),
            _ => false,
        }
    }

    /// A module-level TESTS edge: a MODULE source (the heuristic tier's).
    fn is_module_tests(&self, e: &Edge) -> bool {
        e.category == edge_category::TESTS && self.kind.get(&e.from) == Some(&node_kind::MODULE)
    }

    /// A TESTS edge the walk leaves out (module docs): a module pairing, or a
    /// function-level edge derived from one ([`is_derived_fn_tests`]).
    fn is_pairing(&self, e: &Edge) -> bool {
        self.is_module_tests(e) || is_derived_fn_tests(e)
    }

    /// The MODULEs on `seed`'s nav parent chain, nearest first (the seed
    /// itself when it is one), each with the nodes below it on the chain in
    /// seed-first order: `[seed, parent, ..]` up to the module's child. Stops
    /// at a repeated node, so a malformed cyclic chain cannot loop.
    fn module_chain(&self, seed: NodeId) -> Vec<(NodeId, Vec<NodeId>)> {
        let mut out = Vec::new();
        let mut below: Vec<NodeId> = Vec::new();
        let mut seen: HashSet<NodeId> = HashSet::new();
        let mut at = seed;
        while seen.insert(at) {
            if self.kind.get(&at) == Some(&node_kind::MODULE) {
                out.push((at, below.clone()));
            }
            below.push(at);
            match self.parent.get(&at) {
                Some(&p) => at = p,
                None => break,
            }
        }
        out
    }
}

/// ORIGIN provenance `test_fixture` (`passes::tag_synthetic_provenance`).
fn is_test_fixture(cells: &[glia_core::Cell]) -> bool {
    cells
        .iter()
        .filter(|c| c.kind == cell_type::ORIGIN)
        .any(|c| match &c.payload {
            CellPayload::Json(s) | CellPayload::Text(s) => {
                serde_json::from_str::<serde_json::Value>(s)
                    .ok()
                    .is_some_and(|v| {
                        v.get("provenance").and_then(serde_json::Value::as_str)
                            == Some("test_fixture")
                    })
            }
            _ => false,
        })
}

/// The walk's graph (module docs): every edge of the merge minus the edges
/// into a test case, the module-level TESTS edges and the function-level
/// TESTS edges derived from them. The category filter is the `Adjacency`'s.
struct ReachSource<'a> {
    merged: &'a MergedGraph,
    idx: &'a TestIndex,
}

impl GraphSource for ReachSource<'_> {
    fn node_ids(&self) -> Vec<NodeId> {
        GraphSource::node_ids(self.merged)
    }

    fn edges(&self) -> Box<dyn Iterator<Item = &Edge> + '_> {
        Box::new(
            self.merged
                .all_edges()
                .filter(|e| !self.idx.is_case(e.to) && !self.idx.is_pairing(e)),
        )
    }
}
