# R1 — the Datalog rule layer, which is also THE query surface (0.5.2 bet)

Research only, 2026-09-30, glia at `2170ff8` (v0.5.0). Scope source: `dev-notes/next-leap-0.5.1.md` §0 (GQL /
Cypher read folded into this bet, 2026-09-30), §2 (the 0.5.1 work assumed to exist), §4 (the bet row: "2–3.5k (+ GQL
front-end)", unknown = a runtime semi-naive evaluator; derived predicates on demand vs stored). Line numbers are at
`2170ff8`; find symbols by name.

Spike: `research/spike-datalog/` (std-only Rust, 530 lines; evaluator core ≈ 285 lines, `src/main.rs:1-285`) — a
runtime-IR semi-naive evaluator with stratified negation and a count aggregate, run over `glia merge --out` dumps of
glia itself and grpc-go (git-archive copies under `research/probe/r1-*`, `GLIA_NO_PERSIST=1 ./target/debug/glia`).

---

## 0. The answer in five lines

1. **Evaluator: hand-rolled**, in a new domain-free crate `rules/` (`glia-rules`): datafrog-style storage (sorted,
   deduplicated batches; stable / recent / to-add semi-naive), stratified negation, a native closure builtin that runs
   `activation::algo::reach` instead of generic recursion, Soufflé-style provenance columns `(rule, height)` for
   witnesses, and a 3-level tier lattice. No engine crate fits as-is (§1).
2. **EDB**: a `FactSource: GraphSource` trait in `rules/` — core predicates every domain has (`node/2`, `edge/3`,
   `cell/3`, registry names) plus domain-declared attribute columns; the code domain adds `qname`, `file`, `in_scope`,
   edge tiers from `why::tier_of`. Works for toy-domain (§2).
3. **Check**: `forbid_edge` → a one-rule program (direct) or a seeded closure (`transitive = true`); `no_cycle` stays
   native Tarjan, exposed as a builtin; `invariant` gains an optional `rule` (Datalog text) whose `violation` rows
   must be empty; CC-2 reflexion re-expressed as a shipped prelude with the native 0.5.1 code as the parity oracle
   (§3).
4. **On demand only** in 0.5.2 — query-time, additive, no PARSER_STAMP, no format change; a row's tier is the
   weakest positive input on its best derivation (min over derivations of max over inputs), its evidence the leaf
   edges of its minimal-height derivation (§4).
5. **Smallest first release (R1)**: Datalog text only, `invariant.rule`, `forbid_edge.transitive`, `glia query`
   (Datalog), `PyGraph.query`. GQL / openCypher `MATCH … WHERE … RETURN` subset + aggregates + the CC-2 prelude are
   R2 on the same IR (§5, §7).

---

## 1. The runtime evaluator

### 1.1 Why "runtime" rules out the obvious crates

Rules arrive as text at run time: `[[constraint]]` stanzas in `.glia/overlay.toml` become CONSTRAINT cell entries at
build time (`engine/src/external/declared.rs:28-31`) and `check` reads them back at query time
(`declared_constraints`, `engine/src/external/declared.rs:412-449`); API rules come through `glia cell set` into the
same cell. Anything that compiles rules into Rust at `cargo build` time cannot evaluate them.

### 1.2 Options weighed

| option | runtime rules | negation / aggregation | determinism | dependency cost | verdict |
|---|---|---|---|---|---|
| **ascent 0.8.1** (2026-08-29) / **crepe 0.2.0** (2025-12-14) | no: proc-macro, rules compiled at `cargo build` | yes / yes | yes | proc-macro deps | fails the requirement. Could compile a *fixed* prelude only — two engines for one language, rejected |
| **datafrog 2.0.1** (last release 2019-01-02, 5M downloads, Polonius) | joins are Rust calls, so an IR can drive them, but `Relation<T: Ord>` is statically typed: arbitrary arity means `Vec<u32>` tuples, losing the reason to use it | `from_antijoin` only; no aggregation; stratification by hand | yes (sorted relations) | zero deps, ~1k LOC | adopt the **design** (Variable stable / recent / to_add, sorted merge joins, leapjoin), not the crate: an IR adapter would be as much code as the evaluator |
| **egglog 3.0.0** (2026-08-19) | yes, interprets its own language | no stratified negation in the Datalog sense; lattices via merge functions; no aggregates | e-graph extraction, not designed for byte-identical relation output | 20 non-dev deps incl. 6 own sub-crates, `im-rc`, `ordered-float`, `csv` | wrong tool: equality saturation is the point of it; we would still translate our syntax into its |
| **differential-dataflow 0.25.1** (2026-07-15) | yes: dataflows can be built from an IR at run time (3DF precedent) | antijoin, `reduce` | multi-worker output order varies → sort at the end | timely + its stack; heavy | only if `glia watch` (the other 0.5.2 bet) demands incremental rule maintenance. The spike says recompute is cheap (§1.4) — defer |
| **Nemo** (TU Dresden, Apache-2.0 / MIT) | yes, full: stratified negation, aggregates, existential rules, semi-naive over columnar tries | yes | not stated | not published on crates.io (git dep), RDF / SPARQL I/O stack | reference design only |
| **CozoDB 0.7.6** | yes (CozoScript) | yes | not stated | storage engines; dormant since 2023-12-11 | no |
| **Scallop** (45k LOC Rust) | yes | yes, plus provenance semirings | — | research stack, PyTorch bindings | reference for tier-as-semiring (§4.2) |
| **hand-rolled semi-naive** | yes | ours: stratified negation + grouped aggregates | ours: append-only / sorted relations, `HashMap` for lookup only | none | **recommended** |

URLs: datafrog <https://docs.rs/datafrog/latest/datafrog/>, <https://crates.io/api/v1/crates/datafrog>; ascent
<https://github.com/s-arash/ascent>; crepe <https://github.com/ekzhang/crepe>; egglog
<https://github.com/egraphs-good/egglog>, deps <https://crates.io/api/v1/crates/egglog/3.0.0/dependencies>;
differential-dataflow <https://github.com/TimelyDataflow/differential-dataflow>; Nemo <https://github.com/knowsys/nemo>,
<https://ceur-ws.org/Vol-3801/short3.pdf>; Cozo <https://github.com/cozodb/cozo>; Scallop
<https://github.com/scallop-lang/scallop>, <https://arxiv.org/pdf/2304.04812>; Polonius on datafrog
<https://github.com/rust-lang/polonius/blob/master/polonius-engine/src/output/mod.rs>.

### 1.3 The recommended evaluator, concretely

- **Values**: `u32` — node ids as `Adjacency` dense indices (`activation/src/algo/mod.rs:202-209`, dangling endpoints
  included by its parity rule 1, `mod.rs:95-98`), strings (registry names, qnames, paths) interned per evaluation,
  small ints. A per-evaluation value table, never persisted.
- **Relations**: flat row-major `Vec<u32>` with an arity stride; semi-naive batches as in datafrog (stable sorted
  runs, `recent`, `to_add`); dedup by merge, not a `HashSet<Box<[u32]>>` (one heap box per tuple plus hash indexes is the
  likely bulk of the spike's 887 MB RSS at 10×, §1.4 — unprofiled).
- **Joins**: binary joins, body atoms reordered bound-first by a deterministic greedy planner (Angle leaves ordering
  to the author — "statements are matched top-to-bottom", <https://glean.software/docs/angle/efficiency/>; glia's
  rules will be written by agents, so the planner does it). Worst-case-optimal joins (leapfrog triejoin, Nemo's
  choice) only matter for cyclic bodies (triangles) — not the shape of architecture rules; later if ever.
- **Semi-naive**: per rule with recursive atoms `p1..pk`, one variant per `pi` reading `recent` at `pi`, `stable` at
  `pj<i`, `stable ∪ recent` at `pj>i` (the spike's `fixpoint`, `spike-datalog/src/main.rs:190-283`).
- **Stratification**: Tarjan over the predicate dependency graph (a few dozen predicates; hand-rolled, or reuse
  `activation::algo::cycles::strongly_connected`, `activation/src/algo/cycles.rs:32`, over a tiny `GraphSource`);
  negation and aggregation only across strata ("recursion only under an even number of negations" is CodeQL's rule,
  <https://codeql.github.com/docs/ql-language-reference/recursion/>; glia is stricter: none through negation).
- **Closure builtin** `reaches(x, y, categories)`: when `x` is bound (the forbid-transitive shape), a multi-source
  `reach::bfs` over one `Adjacency` per category set (`activation/src/algo/reach.rs:42`, `:100`), instead of a
  generic recursive predicate. CodeQL ships the same as `+` / `*` operators. Only an unbound `reaches(x, y)` falls back
  to semi-naive (and hits the row budget, below).
- **Row budget**: a derived-row cap (default in the low tens of millions) that aborts the program with an error naming
  the predicate and advising a seed — the same posture as `Adjacency::over_limit`
  (`activation/src/algo/mod.rs:182-185`), never a silent truncation.
- **Determinism**: rules in source order, atoms in planner order, relations sorted or append-only; `HashMap` only
  looked up, never iterated into output (the rule `Adjacency` already states, `activation/src/algo/mod.rs:105`).
  Output rows sorted by RENDERED values (qname, file, line), not by NodeId: NodeIds hash a path-derived RepoId, so id
  order differs between checkouts — `check` already sorts cycle witnesses by qname for this reason
  (`engine/src/check.rs:524-529`). Single-threaded in R1; per-stratum rayon later with an ordered merge.

### 1.4 Measured cost (spike, AMD Ryzen 7 3700X, release build)

EDB = `node/2`, `edge/3`, `carrycat/1` loaded from the dumps; `carry/2` = the edges of the forbid default set
(IMPORTS + the code profile's carry edges, `code-domain/src/profile.rs:83-111`). Scopes approximated by qname prefix
(the dump has no file column).

| query | glia (18,387 nodes / 56,738 edges) | grpc-go (14,221 / 35,426) | glia ×10 + 1% random cross CALLS (183,870 / 573,053) |
|---|---|---|---|
| EDB load | 6–10 ms | 5 ms | 147 ms |
| `carry/2` (1-rule join) | 29,610 rows, 7–8 ms | 18,068 rows, 4.5 ms | 301,773 rows, 83 ms |
| Q1 unseeded TC over CALLS | 242,467 rows, 101–115 ms | 27,823 rows, 10 ms | 3.82 M rows, 2.59 s |
| Q1b unseeded TC over carry | 380,829 rows, 165–176 ms | 50,847 rows, 26 ms | 7.25 M rows, 4.64 s |
| Q2 forbid-transitive, unary seeded (`cli::`→`store::`) | src 829, reached 1,623, hit 118: **3.3 ms** | 1.5 ms | reached 23,622: **16 ms** |
| Q2b same, pairwise `(src, reached)` | 14,452 rows, 7.4 ms | 1.8 ms | 245,611 rows, 166 ms |
| Q3 stratified negation (live / dead FUNCTIONs) | 6 ms | 1.3 ms | 96 ms |
| Q4 grouped count (fan-in) | 1.2 ms | 0.5 ms | 21 ms |
| Q5 3-atom direct forbid join | 2.3 ms | 1.4 ms | 26 ms |
| whole program (all of the above) | 0.32 s, peak RSS 63 MB (all three runs) | 0.08 s | 7.8 s, RSS 887 MB |

Reading it: every check-shaped query is milliseconds at glia / grpc-go size and ≤ 170 ms at 10×; only UNSEEDED
closure is expensive and grows quadratically (7.25 M rows at 10×). So (a) the check compiler must emit seeded forms
and the `reaches` builtin, (b) user rules need the row budget and a planner warning on an unseeded recursive
predicate, (c) incremental maintenance (differential-dataflow, DRed) buys nothing at this scale — recompute wins.
grpc-go's closures are small because Go calls through receivers are mostly unresolved (0.5.1 CA-2, Go receivers from
a call's return type, fixes part of that); expect grpc-go numbers to grow toward glia's after 0.5.1.

---

## 2. The EDB mapping (domain-free)

### 2.1 What the domain-free layers hold today

- `core::Node` carries id, repo, confidence and cells — **no kind, no name** (`core/src/lib.rs:170-175`);
  `core::Edge` carries from / to / category / confidence / cells (`core/src/lib.rs:188-199`); edges have no id, and
  `Edge::key` `(from, to, category)` is not unique — two call sites are two edges (`core/src/lib.rs:180-186`, `:222`).
- Kind is domain-free in the STORE: the container core holds `node_kinds` "every domain has kinds"
  (`store/src/container.rs:79-94`) and the header names every id (`store/src/container.rs:138-146`).
- `GraphSource` gives only `node_ids()` and `edges()` (`activation/src/algo/mod.rs:32-41`); `Adjacency` is the CSR
  index with dense `u32` ids (`mod.rs:106-115`).
- Registry names: `activation::profile::Registries` (`activation/src/profile.rs:39-43`) inside `DomainTables`
  (`profile.rs:196`).
- Code names live in `CodeNav` (`code-domain/src/lib.rs:1314-1343`: `name_by_id`, `qname_by_id`, `kind_by_id`,
  `parent_of`); files come from `Locator` (`engine/src/answers.rs:1084`); toy nodes have NO names — kind + reel index
  only (`toy-domain/src/reel.rs:4-11`, `ReelNav` `:71-75`).

### 2.2 Proposed trait (in `rules/`, depends on `core` + `activation` only)

```rust
pub trait FactSource: GraphSource {
    fn tables(&self) -> &DomainTables;                          // registry names
    fn node_kind(&self, id: NodeId) -> Option<NodeKindId>;
    fn node_cells(&self, id: NodeId) -> &[Cell];
    /// Domain columns beyond the core: code = qname, name, file, line, parent, role, repo, project;
    /// toy = index. Bulk, in node order, so a column loads in one pass.
    fn attributes(&self) -> &'static [AttrDecl];
    fn attr_rows(&self, attr: usize, out: &mut Vec<(NodeId, Value)>);
    /// FACT / DERIVED / HEURISTIC of one edge; a domain that says nothing is DERIVED.
    fn edge_tier(&self, e: &Edge) -> Tier { Tier::Derived }
    /// Named native predicates (code: `in_scope/2`, `scc/3`); none by default.
    fn builtins(&self) -> &[&dyn Builtin] { &[] }
}
```

Core predicates every domain gets: `node(Id, Kind)`, `edge(From, To, Category)` (set semantics; the witness is
located by the first edge in global edge order with that key, as `why` and `implementors` already do,
`engine/src/implementors.rs:22-27`), `edge_conf(From, To, Category, Confidence)`, `cell(Id, CellType, Text)` (Text
and Json payloads; Bytes skipped), `edge_cell(From, To, Category, CellType, Text)`. Kinds / categories / cell types
are interned names from the header registries, so a rule says `node(r, "ROUTE")`, never an id — ids are locked
(`code-domain/src/lib.rs` registries) but a rule stays readable, and an unknown name is a rule error naming it, as
`categories_of` does for check today (`engine/src/check.rs:231-253`).

Materialise lazily: only relations the program references are loaded — the Locator pass for `file` is the costly one
(`check` builds it for every node up front, `engine/src/check.rs:269-298`).

Code binding: `engine/src/rules.rs` (new public slot) implements `FactSource` over `MergedGraph` (+ `Locator`):
attributes from `nav`, `file` from `Locator::file_of`, builtin `in_scope(Id, Path)` with check's STRICT semantics
(file under the path on a segment boundary, or a PROJECT at it; an unlocated node is in no scope,
`engine/src/check.rs:32-38`, `in_scope` `engine/src/answers.rs:1299-1306`), `edge_tier` = `why::tier_of`
(`engine/src/why.rs:478-504`, made `pub(crate)` by 0.5.1's CC prerequisite C).

Toy proof: `toy-domain/tests/rules.rs` implements `FactSource` for `ToyGraph` (`toy-domain/src/reel.rs:190-198`) with
one attribute (`index`) and runs a recursive rule over `NEXT_SHOT`. toy-domain's dependency guard lists exactly the
domain-free layers (`toy-domain/tests/end_to_end.rs:396-408`): `glia-rules` is a domain-free layer, so it joins that
list (one line) — confirm with James (§8).

`rules/` must NOT be in `stamp/build.rs` `HASHED_ROOTS` (`stamp/build.rs:29-30`): it is query-time, like
`projection-text`. Adding the crate still changes `Cargo.lock`, which is hashed (`stamp/build.rs:35`): PARSER_STAMP
moves once at 0.5.2 (declared break, as §2 "Cross-cutting findings" of the 0.5.1 doc requires).

---

## 3. How today's check rules and 0.5.1's CC-2 compile

Today (`engine/src/check.rs`): `forbid_edge` = every DIRECT edge of the rule's categories from scope `from` to scope
`to` (`check.rs:12-20`, `:390-443`), labelled FACT unconditionally (`check.rs:442`, the bug 0.5.1 prerequisite C
fixes); `no_cycle` = SCCs of the module import graph or of the node graph of the listed categories, tier DERIVED
(`check.rs:21-28`, `:468-546`); `invariant` = text, listed unchecked (`check.rs:29-30`, `:200-206`). Kinds:
`ConstraintKind` (`code-domain/src/external_inputs.rs:439-447`), overlay schema `ConstraintDecl`
(`code-domain/src/glia_config.rs:254`), kinds list `CONSTRAINT_KINDS` (`glia_config.rs:64`).

| rule | compiles to | notes |
|---|---|---|
| `forbid_edge {from, to, categories}` | `violation(a, b) :- edge(a, b, c), cat(c), in_scope(a, FROM), in_scope(b, TO).` | parity oracle = today's `forbid_edge` fn; the "scope no node sits in" error (`check.rs:316-325`) stays a pre-check |
| `forbid_edge … transitive = true` (new field) | `seed(a) :- in_scope(a, FROM).` `hit(b) :- seed(a), reaches(a, b, CATS), in_scope(b, TO).` | `reaches` = multi-source BFS; witness = BFS parent chain = shortest path from the nearest seed; tier = weakest hop on the BEST path (3-level BFS, §4.2). Spike: 3.3 ms on glia |
| `no_cycle {scope, categories}` | builtin `scc(Id, Comp, CATS)` computed by `algo::cycles::strongly_connected` + `witness_cycle` (`activation/src/algo/cycles.rs:32`, `:113`); `violation(c) :- scc(x, c, CATS), in_scope(x, S).` | SCC is not Datalog-efficient (mutual `reach` is quadratic); keep Tarjan native, expose it |
| `invariant {text}` | unchanged: unchecked | a statement stays a statement |
| `invariant {text, rule}` (new field) | the rule text, parsed at CHECK time; its `violation/N` rows are the violations | e.g. `handled(r) :- edge(r, _, "HANDLED_BY"). violation(r) :- node(r, "ROUTE"), !handled(r).` |
| CC-2 reflexion (0.5.1: `[[component]] name, paths`, optional `[[layer]]`, `kind="allow"`) | EDB facts `component_path(C, P)`, `allowed(C1, C2)`, `layer(C, N)` + a shipped prelude: `maps(n,C) :- component_path(C,P), in_scope(n,P).` `dep(C1,C2) :- edge(a,b,c), forbidcat(c), maps(a,C1), maps(b,C2), C1 != C2.` `convergence(C1,C2) :- dep(C1,C2), allowed(C1,C2).` `divergence(C1,C2) :- dep(C1,C2), !allowed(C1,C2).` `absence(C1,C2) :- allowed(C1,C2), !dep(C1,C2).` `mapped(n) :- maps(n,_).` `unmapped(n) :- located(n), !mapped(n).` `layer_break(C1,C2) :- dep(C1,C2), layer(C1,L1), layer(C2,L2), L1 < L2.` | CC-2's native code becomes the ORACLE: a parity test runs both on fixtures + quokka and diffs, the pattern LD.15a used for `reach` (its tests hold the old scan loops as oracle, `activation/src/algo/mod.rs:13-16`). Flip to the prelude only when identical |

Parse the rule at check time, not at overlay load: the loader drops an invalid stanza (`glia_config.rs:13-21`), which
would make graph contents (the CONSTRAINT cell) depend on the rule parser and drag `rules/` into PARSER_STAMP. At
check time a bad rule is an `errors` row (exit 2, `CheckReport::errors`, `engine/src/check.rs:130-132`) — the
existing posture for an unknown category.

Severity stays VIOLATION for a declared rule (`engine/src/check.rs:70-72`); a pattern-derived observation stays a
DIVERGENCE (`engine/src/patterns.rs`) and never goes through `check`.

---

## 4. On demand vs stored; tiers and evidence through a derivation

### 4.1 On demand (recommended for 0.5.2)

Rules are evaluated at `check` / `query` time over the loaded or merged graph: additive (API + CLI + py), no graph
contents change, no FORMAT_VERSION bump, no PARSER_STAMP coupling, and `--with` merges just work (derive over the
merged graph, so Glean's stacked-DB ownership complication never arises). Long-running consumers (repo-graph's MCP
server, a `PyGraph`) may memoise results in process, keyed by (layout manifest fingerprint, program hash) — never in
the `.gmap`.

### 4.2 Stored (not in 0.5.2; what it would cost)

A rule that DERIVES EDGES for other answers (blast radius, trace) to follow would be a `Stage::Post` `PassSpec`
(`activation/src/passes.rs:32-39`, `:54-66`) in `CODE_PASSES`, emitter `pass:rules` (the `pass` stage already
exists, `code-domain/src/evidence.rs:51-60`, tier DERIVED by `stage_tier`, `engine/src/why.rs:468-475`). Costs: the
evaluator enters `HASHED_ROOTS`; a user rule cannot mint a new edge category (ids are allocated only in
`code-domain/src/lib.rs`, locked once allocated), so stored rules could only emit existing categories or a generic
cell; rule edits change graph contents. Glean's answer is the model if ever: stored derived predicates are
materialised views computed by an explicit `glean derive` before `finish`, and (in the fork that added recursion)
stored predicates may not be recursive (<https://glean.software/docs/derived/>,
<https://github.com/lazamar/Glean/pull/14>).

### 4.3 Tiers

A tier is an element of the finite chain FACT < DERIVED < HEURISTIC (ordered by weakness). Propagation is the
"security / clearance" style semiring: conjunction (a rule body) = **max** (weakest input), alternative derivations
= **min** (best derivation). It is commutative, idempotent and absorptive, so semi-naive terminates and each tuple's
tier can improve at most twice (Green, Karvounarakis, Tannen, *Provenance Semirings*, PODS 2007,
<https://www.cis.upenn.edu/~plclub/propr/greg-slides.pdf>; Datalog specifics: Bourgaux et al., *Revisiting Semiring
Provenance for Datalog*, KR 2022, <https://arxiv.org/pdf/2202.10766>; Scallop generalises this to arbitrary
semirings). Implementation: a tier column with lattice semi-naive (a tuple re-enters `recent` when its tier
improves); for the `reaches` builtin, three BFS passes (FACT edges only, then +DERIVED, then all) — a node's tier is
the first pass that reaches it, exact and O(3(V+E)).

Answer to "a derived row's tier = weakest input?": **the weakest input on its BEST derivation**, not on the first one
found. Otherwise a HEURISTIC short path would mask a FACT long path to the same row. Inputs: edge tier from the domain
(`why::tier_of`); node / attribute / `in_scope` facts are FACT (read at a site); negated atoms contribute no tier —
the row instead carries the coverage caveats of the categories the negated predicate reads, LD.8a's absence model
(`engine::absence`). Aggregates (R2): the weakest contributing row — open question 5.

Glean's matching rule: a derived fact's owner is the conjunction of its inputs' owners (`O1 && … && On`) and ownership
sets are interned because few distinct sets exist
(<https://glean.software/docs/implementation/incrementality/>) — the same shape as tier = max, and as CD-7's EVIDENCE
interning.

Two tier vocabularies exist today and must converge before rules can report tiers uniformly: `why` tiers by emitter
stage (`engine/src/why.rs:468-504`, lowercase `"fact"` consts at `why.rs:75`), `check` by lowercase consts
(`engine/src/check.rs:74-76`), `implementors` by edge CONFIDENCE with an uppercase enum (`engine/src/implementors.rs:
127-148`, Strong→FACT). The rules layer takes the `why` definition through `FactSource::edge_tier`; a domain-free
`Tier` enum (Ord, max = weakest) belongs in `activation` or `core` — ideally landed by 0.5.1 with prerequisite C.

### 4.4 Evidence

Soufflé's provenance: annotate each IDB tuple with the rule that derived it and its proof height, then build
minimal-height proof trees lazily by search, with no re-evaluation (Zhao, Subotić, Scholz, TOPLAS 2020,
<https://arxiv.org/abs/1907.05045>). glia adopts exactly that: two `u16` columns `(rule, height)` per derived tuple
(height = the semi-naive round). `explain(row)` searches for body tuples of lower height matching the rule, recursing
to EDB edges; each leaf edge is located through its EVIDENCE cell (`code-domain/src/evidence.rs:87-95`, 0-based line
converted to 1-based only at the `Locator` boundary, LD.1) and rendered as today's `ViolationEdge`
(`engine/src/check.rs:83-96`), capped at `MAX_EVIDENCE` (`check.rs:78`). The witness is the minimal-height
derivation — for forbid-transitive, a shortest path; tie-break by rendered qname for checkout-stable output.

---

## 5. The GQL / openCypher read front-end

Scope: read-only `MATCH … [WHERE …] RETURN [DISTINCT] … [ORDER BY …] [LIMIT n]`, lowered to the same IR.

| construct | lowering |
|---|---|
| `(a:ROUTE)` | `node(a, "ROUTE")` |
| `(a {qname: 'x'})`, `a.file`, `a.qname` | attribute atoms from `FactSource::attributes` |
| `-[:CALLS\|USES]->` | `edge(a, b, c), c ∈ {CALLS, USES}` |
| `->{1,}` (GQL) / `-[:T*1..]->` (Cypher) / `+` | `reaches(a, b, CATS)` (native BFS) |
| `->{1,3}` | bounded unrolling, or BFS with `max_depth` (`reach::bfs` takes one, `reach.rs:42`) |
| `WHERE` AND / OR / NOT / `=` `<>` `<` `STARTS WITH` `CONTAINS` `IN [..]` | conjunction; OR → one rule per disjunct (DNF); comparisons as builtins |
| `NOT (a)-[:HANDLED_BY]->()` / `NOT EXISTS {…}` | an auxiliary predicate + a negated atom (next stratum) |
| `in_scope(a, 'web')` | the domain builtin, as a function call |
| `RETURN a.qname, count(b)` | head projection; aggregate = grouped operator (R2) |
| `p = ANY SHORTEST (a)-[..]->+(b)` | the BFS witness, returned as a located path |
| `ORDER BY`, `LIMIT` | post-processing on rendered rows |

Deliberate deviations, documented: set semantics (rows DISTINCT) and edge homomorphism, like Datalog — openCypher uses
relationship isomorphism within a pattern, and GQL requires every unbounded quantifier to sit under a restrictor
(TRAIL / ACYCLIC / SIMPLE) or a selector (ANY / ALL SHORTEST) because it returns paths; glia returns endpoint pairs, so
`->{1,}` is plain reachability and only a path variable needs `ANY SHORTEST`. Conformance is a non-goal: engines
already disagree on nine of seventeen path-pattern constructs (Mandarapu, Kunkunuru 2026,
<https://arxiv.org/abs/2609.23032>). Call it "a GQL read subset", never "GQL". Prior art for Cypher → Datalog IR:
Raqlet (Cypher / SQL-PGQ → PGIR → DLIR → SQIR, magic sets on DLIR; Shaikhha et al. 2025,
<https://arxiv.org/abs/2508.03978>); openCypher in relational algebra (<https://arxiv.org/pdf/1705.02844>). Standard:
ISO/IEC 39075:2024 (<https://en.wikipedia.org/wiki/Graph_Query_Language>).

Grammar: **hand-rolled** — one shared lexer for both surfaces, recursive descent for patterns, Pratt for `WHERE`
expressions. winnow 0.7.15 is in the lock (`Cargo.lock:2201-2202`, pulled by `toml_edit`), so using it adds no new
package; but toml moving to winnow 1.x (1.0.2 already in the local registry cache) would then carry two versions, and
error spans / messages are what users see most. Winnow saves perhaps a third of the parser lines; either is fine —
the recommendation is hand-rolled for span control and zero version coupling.

---

## 6. Glean / Angle lessons

1. **On-demand derived predicates as abstraction layers** (<https://glean.software/docs/derived/>): glia ships a
   prelude of on-demand predicates (`carry`, `entry`, `live`, `effect`, `service_of`, the CC-2 reflexion set) so
   user rules name concepts, not raw categories; when the EDB changes (a category split, CD-7's EVIDENCE interning)
   the prelude changes and user rules do not.
2. **Stored = materialised view, computed by an explicit step** (`glean derive` before `glean finish`), and stored
   predicates are non-recursive (lazamar fork). If glia ever stores, same rules: an explicit pass, non-recursive only.
3. **Index by key prefix; statement order matters** (<https://glean.software/docs/angle/efficiency/>): glia's planner
   orders atoms bound-first itself, because agents write the rules.
4. **Recursion bottom-up with magic sets, one demand predicate per binding pattern**, results streamed per round;
   counting to 80,000 one number per round: 141 s → 111 ms (<https://github.com/lazamar/Glean/issues/13>, PR #14 — a fork, not merged
   upstream). glia R1 gets the same effect for the common case by hand (the check compiler emits seeded forms, the
   `reaches` builtin); a general magic-sets rewrite is R2.
5. **Derived-fact ownership = conjunction of input owners, interned ownership sets**
   (<https://glean.software/docs/implementation/incrementality/>): the tier semiring and the evidence leaf set are the
   same idea; for `glia watch`, a derived row is stale only when a leaf's evidence file changed — available free from
   §4.4's provenance if incremental maintenance is ever needed.
6. **Stacked databases complicate incremental derivation** (new ownership induced in lower DBs; Glean lists
   incremental derivation over stacked DBs as unimplemented): glia derives over the merged graph at query time, so a
   `--with` merge is never stacked.

---

## 7. Recommendation and releases

**R1 (0.5.2 first release) — the smallest that makes `invariant` checkable and `forbid_edge` transitive:** Datalog
text only; runtime evaluator with stratified negation, comparisons, `in_scope`, `reaches`, `scc`; provenance witness
+ tier lattice; `invariant.rule`; `forbid_edge.transitive`; `glia query` (Datalog) and `PyGraph.query`, because once
the evaluator exists the read surface is 250–450 lines and the bet names this layer THE query surface.

**R2:** GQL read subset → IR; grouped aggregates (count / min / max / sum); the CC-2 prelude with native parity;
the shipped predicate prelude; a magic-sets / demand rewrite for unseeded user recursion.

**R3 (only on James's call):** in-process memoisation for long-running consumers; stored derived edges as a
`pass:rules` Post pass (enters PARSER_STAMP); provenance-driven invalidation for `glia watch`.

### Components and LOC

| component | crate | release | LOC |
|---|---|---|---|
| IR, values, interning, builtin registry | `rules/` (new, domain-free) | R1 | 200–300 |
| shared lexer + Datalog parser with spans | `rules/` | R1 | 400–600 |
| analysis: safety / range restriction, arity, stratification, negation checks | `rules/` | R1 | 250–400 |
| planner: bound-first order, index choice, semi-naive variants | `rules/` | R1 | 150–300 |
| evaluator: batches, merge joins, negation, builtins, row budget | `rules/` | R1 | 450–700 |
| `reaches` / `scc` builtins over `algo::reach` / `algo::cycles`, 3-level tier BFS | `rules/` | R1 | 100–180 |
| provenance `(rule, height)`, `explain`, tier lattice | `rules/` | R1 | 250–400 |
| `FactSource` trait + lazy EDB loader + registry names | `rules/` | R1 | 150–250 |
| `rules/` tests: oracle vs `reach` / `cycles`, determinism, budget, errors | `rules/` | R1 | 500–750 |
| toy proof `tests/rules.rs` + guard line | `toy-domain/` | R1 | 80–150 |
| `ConstraintKind` fields (`rule`, `transitive`), `constraint_rule`, `validate_entry`, `ConstraintDecl` + tests | `code-domain/` | R1 | 120–220 |
| declared.rs carries the new fields into entries | `engine/` | R1 | 40–80 |
| `engine::rules` slot: `FactSource` for `MergedGraph` (+ Locator), `in_scope`, `edge_tier`, located rows | `engine/` | R1 | 300–450 |
| `check` integration: invariant rules, transitive forbid, per-violation tier, marker counts | `engine/` | R1 | 180–320 |
| engine tests (fixtures: a rule per kind, merged `--with`, byte-identical output) | `engine/` | R1 | 300–450 |
| `glia query` (Datalog file / `-e` / stdin, `--with`, `--json`) + `cli/surface/query.txt` | `cli/` | R1 | 170–300 |
| `PyGraph.query` + `py/api_surface/query.txt` + surface test | `py/` | R1 | 80–150 |
| **R1 total** | | | **≈ 2.8–4.7k prod + 0.9–1.35k tests** (the §4 "2–3.5k" row did not price parser diagnostics, provenance or tiers) |
| GQL read subset parser | `rules/` | R2 | 450–700 |
| GQL → IR lowering (DNF, NOT patterns, quantifiers, projections) | `rules/` | R2 | 300–450 |
| grouped aggregates as a stratum operator | `rules/` | R2 | 150–250 |
| magic-sets / demand rewrite | `rules/` | R2 | 350–550 |
| predicate prelude (`include_str!` rule text) + CC-2 prelude + parity oracle | `engine/` | R2 | 350–650 |
| CLI / py additions (`--gql`, `PyGraph.query_gql` or auto-detect) | `cli/`, `py/` | R2 | 100–200 |
| R2 tests | all | R2 | 500–800 |
| **R2 total** | | | **≈ 1.7–2.8k prod + 0.5–0.8k tests** |
| memoisation / stored `pass:rules` / watch invalidation | `engine/`, `rules/` | R3 | 1.2–2k, only if chosen |

### Needs from 0.5.1 first

- **CC prerequisite C** (`why::tier_of` → `pub(crate)`, check tiers forbid_edge rows by it): the rules layer's
  `edge_tier` IS that function. Best if 0.5.1 also lands ONE domain-free `Tier` enum (Ord, max = weakest) replacing
  the three vocabularies (`why.rs:75`, `check.rs:74-76`, `implementors.rs:127-148`).
- **CC-2 reflexion** (0.5.1): its overlay schema (`[[component]]`, `[[layer]]`, `kind="allow"`) defines the EDB facts
  of the R2 prelude and its native implementation is the parity oracle. Ask CC-2's packet to keep component
  membership a pure function of (node, path) so the prelude can reproduce it exactly.
- **CC-3 `glia review`** runs `check` on both sides of one RevDelta: rule-based violations then appear as new /
  resolved for free, provided R1 keeps the `check` API shape. No blocker.
- **CD-7 FORMAT_VERSION 3** (EVIDENCE interning, CODE spans): rules read evidence only through `Evidence::of` — take
  whatever API CD-7 leaves; no ordering constraint beyond "after".
- Nothing in 0.5.1 must reserve a registry id: rules mint no kinds, categories or cells (next free stay node_kind 50,
  edge_category 37, cell_type 26).
- 0.5.2's own wave 0 declares the slots: `rules/` crate + workspace member, `engine::rules` (public),
  `cli/src/cmd/query/query.rs` + variant, `py/src/query.rs`.

---

## 8. Open questions only James can answer

1. **Rule language for `[[constraint]]`**: Datalog text only (GQL is the ad-hoc read surface), or may an invariant be
   a GQL `MATCH … RETURN` that must return no rows?
2. **Does a violation whose best witness is HEURISTIC fail CI** (exit 1)? Options: always; never (report only); a
   per-rule `min_tier`.
3. **Transitive forbid semantics**: does a path through a third scope count (web → shared → api/internal)? Is there
   an allowed-facade escape (`via = ["api/public"]`)? Does IMPORTS (module-level) chain with CALLS (symbol-level) in
   one closure, or does a transitive rule take one level?
4. **Set semantics + edge homomorphism** for the GQL subset, knowingly unlike openCypher's isomorphism — acceptable?
5. **Tiers across negation and aggregation**: a row produced through `!p` — its positive atoms' tier plus absence
   caveats (proposed), or always FACT like LD.8a's absence reason? An aggregate — the weakest contributing row?
6. **SECURITY.md** (`SECURITY.md:36-47`): a general query surface lets a repo owner write reachability questions the
   gated list names ("routes that reach data without authentication"). glia would ship no auth / security predicate in
   the prelude and no value-flow EDB exists (structural edges only, like `effects` today). Is a user-authored query
   language acceptable under "every new kind of answer is checked against the list"?
7. **Overlay compatibility**: every overlay struct is `deny_unknown_fields`, so an overlay using `rule` /
   `transitive` makes 0.5.1 default the WHOLE config (`glia_config.rs:13-21`). Accept (consumers pin exactly, §0) or
   bump the overlay to `version = 2`?
8. **Stored derived edges ever** (rules as inference that blast radius / trace follow)? If yes, the evaluator becomes
   build-affecting and joins PARSER_STAMP.
9. **Rules via the cell API and `origin = "llm"`**: may `glia cell set` / an LLM write rule text into CONSTRAINT
   entries, or overlay-only?
10. **toy-domain guard**: add `glia-rules` to the toy's allowed domain-free dependency list, or keep the toy proof as a
    `rules/` test with an inline toy `FactSource`?

## 9. Spikes worth running before packets

1. **Evaluator on the planned IR with provenance**: port the spike to sorted batches + `(rule, height)` columns;
   measure overhead vs the spike on glia, grpc-go (after 0.5.1's Go fixes) and quokka; gate: 20 check rules < 200 ms
   on glia, RSS < 150 MB at glia ×10.
2. **EDB extraction on `MergedGraph`** incl. `Locator::file_of` for every node: decides lazy per-column loading.
3. **Tier lattice**: a mixed-tier fixture where the first-found path is HEURISTIC and a longer path is FACT; 3-pass
   BFS vs lattice semi-naive must agree.
4. **CC-2 parity** once CC-2 lands: prelude vs native on fixtures + quokka, byte-identical rendered output.
5. **Witness stability across checkouts**: build glia at two paths, run one transitive rule, diff rendered witnesses
   (NodeIds differ with the path; tie-breaks must be by rendered values).
6. **GQL subset coverage**: 30 real questions (repo-graph MCP usage, README examples) → how many the subset expresses;
   pick quantifier spellings.

## 10. Risks

- **Unseeded recursion blow-up** — measured 7.25 M rows / 4.6 s / 887 MB at glia ×10. Mitigations: `reaches`
  builtin, seeded compilation of check kinds, row budget with an actionable error, a planner warning; R2 magic sets.
- **Semantic drift** between native check kinds and compiled rules — keep native code as oracle until parity holds;
  then delete, not keep two.
- **Determinism** — witness choice depends on edge order, which the Finalize sort ties to NodeId values
  (`core/src/lib.rs:261`), path-dependent across checkouts; sort outputs and break ties by rendered values; no
  `HashMap` iteration; single-threaded R1.
- **Scope creep into a general graph database** (OPTIONAL MATCH, path returns, writes, `CALL`) — the subset list in §5
  is the contract; anything else is a new packet.
- **Rule rot** — a rule naming a removed kind / category / predicate must error loudly (exit 2), and `glia gaps`'
  overlay-rot report should list it (the LF.2c pattern for orphaned stanzas).
- **Security gate** — open question 6.
- **Tier inconsistency** — three vocabularies today; rules would expose all of them unless unified first.
- **Overlay schema break** — open question 7.
