# glia 0.5.2 — the four bets

Scoped 2026-10-02 at `84e2e71` (v0.5.1 tagged, `glia-py` 0.5.1 on PyPI the same day). Sources: the 0.5.1 plan
(`dev-notes/next-leap-0.5.1.md` §0, §4 and §7.4), the research notes `dev-notes/research-0.5.2/` (R1-datalog,
R2-watch, R2-identity, R2-engram with Engram's 2026-10-02 answer), and two findings from releasing 0.5.1 (memories
`project_engine_rss_creep`, `project_generated_grpc_stub_false_calls`). Line numbers in the research notes are at
0.5.1-era commits and drift: find symbols by name.

## 0. Decisions (verbatim)

- 2026-09-19 / 09-30 (0.5.1 plan §0): *"5.2 / Top 3 bets then"*; 0.5.2 = four bets: `glia watch`, the Datalog rule
  layer, cross-repo identity, Engram PPR memory. GQL / Cypher read is folded into the Datalog layer (one query
  surface). `decide/` a–e and `glia judge` stay in 0.5.4.
- 2026-10-01 (0.5.1 plan §7.4), deferred here: PHP short-name symbol imports and `use \App\{X}`; CB.26's measured
  residuals; the rest of the matrix none / partial cells; WS / gRPC / GraphQL external-endpoint marking. The
  `patterns_experimental` / `patterns_vs_rev_experimental` aliases and the hidden `--experimental` flag were promised
  "until 0.5.2".
- 2026-10-02: *"maybe ask engram what it needs right now and update that section then i'll compact and you can finish
  specing"*. Engram answered (R2-engram.md, "Engram's answer, 2026-10-02"): the fourth bet becomes **Engram anchors**
  (below); no PPR default read path.
- Version mechanics (0.5.1 rule): the name is 0.5.2, consumers pin exactly, breaks are allowed when a packet declares
  them, and the version bump is the last step before the tag (*"Why the fuck do we need to bump a version while we
  are still building"*).

### 0.1 The open questions, resolved by default

The research notes asked 25 questions. Q1–19 were put to James with my defaults on 2026-10-02 and not overridden
("you can finish specing"); Engram answered Q20–25 itself. Any default can still be overridden before the wave that
implements it lands.

| # | question | resolution |
|---|---|---|
| 1 | rule language for `[[constraint]]` | Datalog text only; GQL is the ad-hoc read surface |
| 2 | does a HEURISTIC-witnessed violation fail CI | per-rule `min_tier`, default DERIVED; a HEURISTIC row is reported, never exit 1 |
| 3 | transitive forbid semantics | a path through a third scope counts; `via = [...]` names allowed facades; IMPORTS and CALLS chain separately unless a rule names both |
| 4 | GQL set semantics + edge homomorphism | accepted, documented as unlike openCypher's isomorphism |
| 5 | tiers across negation / aggregation | a negated row takes its positive atoms' tier plus the absence caveats; an aggregate takes its weakest contributing row |
| 6 | SECURITY.md vs a user query language | allowed: structural predicates only, no auth / security predicate in the prelude, no value-flow EDB; a SECURITY.md paragraph says so |
| 7 | overlay compatibility for `rule` / `transitive` | overlay `version = 2`; a version-1 overlay keeps loading |
| 8 | stored derived edges | none in 0.5.2 (rules are query-time, outside PARSER_STAMP) |
| 9 | rules via the cell API / `origin = "llm"` | overlay-only; LLM-proposed rules go through the overlay loop (`overlay propose / try / accept`) |
| 10 | toy-domain guard | the toy proof is a `rules/` test with an inline `FactSource`; the toy's dependency list is unchanged |
| 11 | change detection for `glia watch` | stat-poll of the walked file table plus directory re-lists; no `notify` crate |
| 12 | `--persist` cadence | persist on exit and on an interval (`--persist-every <s>`), not after every update |
| 13 | multi-repo watch | single-repo in 0.5.2; `--with` in 0.5.3 |
| 14 | repo-graph adopts `WatchSession` | yes, a handoff item for its session |
| 15 | body-only fast path (A2) | 0.5.3, with Stack Graphs |
| 16 | SHARES_DEPENDENCY / SHARES_SCHEMA's message half vs SAME_AS | replaced by SAME_AS stars (one declared edge_removal break) |
| 17 | collapse classes by default | yes for FACT / DERIVED classes, with `also_in`; `--no-collapse` shows the raw rows |
| 18 | vendored / diverged copies | stay separate rows, reported as same-as candidates (HEURISTIC, never collapsed) |
| 19 | org-internal package routing | 0.5.3 |
| 20 | SAME_AS in Engram | moot: Engram's eval is single-repo and it needs no SAME_AS; engram-export maps or skips the category without a contract change |
| 21–25 | Engram's five (C2, PPR default, seeding, side files, lexical vs dense) | answered by Engram: no PPR default read path; a candidate source enters its pool as a weight-0 feature and stays only if it beats ablation; side files as data only; no v7, cap_prose stays 500 bytes |

## 1. The train

| release | contents |
|---|---|
| **0.5.2** bets | this doc: Datalog rules + GQL (DA, DB), `glia watch` (DC), cross-repo identity (DD), Engram anchors + engine hygiene (DE), the language deferrals (DF), the matrix rest (DG, DH) |
| **0.5.3** scale | Stack Graphs-style incremental resolution (+ watch's A2 body-only fast path), build-target graphs, multi-repo `glia watch --with`, org-internal package routing |
| **0.5.4** proof | precision rate, agent before / after, big-repo timings, calibrated confidence, `decide/` a–e, `glia judge` |

## 2. 0.5.2 scope

Packet ids: `D<group>.<n>[letter]`; `D0.*` is wave 0 (the id allocation, the slots, at most one dependency commit),
written by the integrator. Break kinds as in 0.5.1 (none, qname_shape, node_id, edge_removal, cell_value,
api_signature, format, cli_output, out_of_repo).

### DA — the rules crate (Datalog evaluator; R1-datalog §1–§4)

A new domain-free crate `rules/` (`glia-rules`, depends on `core` + `activation` only):
- IR, values, interning, builtin registry;
- a lexer + Datalog parser with spans and diagnostics;
- analysis (safety / range restriction, arity, stratification, negation checks);
- a planner (bound-first order, index choice, semi-naive variants);
- a datafrog-style evaluator (sorted deduplicated batches, merge joins, stratified negation, comparisons, a row
  budget with an actionable error);
- `reaches` / `scc` builtins over `algo::reach` / `algo::cycles` with the 3-level tier BFS;
- provenance `(rule, height)`, `explain`, the tier lattice (a row's tier = min over derivations of max over inputs);
- grouped aggregates (count / min / max / sum) as a stratum operator; a magic-sets / demand rewrite for unseeded
  recursion;
- the `FactSource: GraphSource` trait (core predicates `node/2`, `edge/3`, `cell/3`, registry names, plus domain
  attribute columns) and the toy proof as an inline-`FactSource` test (Q10).

Gates: oracle tests against `reach` / `cycles`, determinism (no HashMap iteration; ties broken by rendered values,
R1 §10), the measured blow-up case (7.25 M rows at glia ×10) stopped by the budget, the spike's cost targets (20
check rules < 200 ms on glia; RSS < 150 MB at glia ×10).

### DB — rules in the engine: check, query, GQL, prelude (R1-datalog §3, §5, §7)

- `ConstraintKind` gains `rule` (invariant) and `transitive` + `via` (forbid_edge), overlay `version = 2` (Q7),
  `validate_entry` errors for a rule naming an unknown kind / category / predicate (exit 2), the rot report in `glia gaps`.
- `engine::rules` slot: `FactSource` for `MergedGraph` (+ Locator, `in_scope`, `edge_tier` through `why::tier_of`),
  located rows.
- `check`: invariant rules, transitive forbid (Q3), per-rule `min_tier` (Q2), per-violation tier and witness; `review`
  shows rule violations new / resolved through the unchanged `check` API.
- `glia query` (Datalog file / `-e` / stdin, `--gql`, `--with`, `--json`) and `PyGraph.query` / `query_gql`.
- The GQL read subset (`MATCH … WHERE … RETURN`, aggregates) lowered to the same IR (Q4).
- The predicate prelude (`include_str!` rule text) and the CC-2 reflexion model re-expressed as a prelude with the
  native 0.5.1 code as the parity oracle; native code deleted only once parity holds byte for byte.
- SECURITY.md paragraph (Q6). No security predicate ships.

### DC — `glia watch` (R2-watch §6–§9: A1)

- Build-stage refactor: walk reuse, per-repo assembly over an injected file set, a callable per-group build,
  reassembly in clean order.
- `engine::watch` `WatchSession` (`open` / `apply` / `poll` / `result` / `persist` / `save_cache`; `WatchUpdate` with
  `phase_ms` from CA.9's timers), durability classes, the stat-poll file table (Q11), group fingerprints + cache,
  scoped `located_delta`, `[watch]` markers.
- `glia watch <repo> [--persist-every <s>] [--poll-ms N] [--debounce-ms N]`: NDJSON per update on stdout, exits on
  SIGINT / stdin EOF, persists on exit and on the interval (Q12); no daemon, socket or pidfile.
- pyo3 `glia_py.WatchSession` (`apply` / `poll` / `snapshot` / `persist`).
- Speed-ups every build gets: graft memos, the queue const fold and RPC needles on the pool / cached (R2-watch §14),
  the evidence fill split (intra at group build, cross at Finalize), byte-identical.
- The parity gate `engine/tests/watch_parity.rs`: scripted edit sequences per input class plus seeded random ones;
  after every step the persisted layout equals a clean build byte for byte.

### DD — cross-repo identity (R2-identity §5–§7)

- SAME_AS: the one registry allocation of 0.5.2 (edge_category 37, D0.1), not a carry edge.
- `activation::algo::classes`: union-find (min-id representative, path compression) + a `Quotient<G: GraphSource>`
  adapter.
- code-domain `identity_rules` rows in `CODE_TABLES` (kind pairs, key, tier) and evidence rule names.
- `graph/src/resolvers/identity.rs` IdentityResolver, the last Resolve PassSpec: packages, package ↔ project,
  proto messages (with `diverged`), contract ops (shared op-key normaliser with CC.8), SCIP package + descriptor keys
  where both repos are indexed; vendored copies as HEURISTIC same-as candidates, never collapsed (Q18); `[identity]`
  marker.
- Replace PackageResolver's SHARES_DEPENDENCY clique and SHARES_SCHEMA's message half with SAME_AS stars (Q16,
  edge_removal; carry-edge interplay checked so blast radius does not double-walk).
- Answer collapse with `also_in: [{repo, file, line}]` and the class tier in find / blast / trace / tests_for /
  serves / arch (Q17), `--no-collapse`; `glia identity` report and pyo3 `identity_classes()`.
- Gates: two-repo fixtures (shared npm package, a proto message with one diverged copy, a vendored subtree, an npm
  workspace lib), merge == joint build byte for byte with SAME_AS present.

### DE — Engram anchors and engine hygiene

- **Cross-clone export determinism** (Engram rank 1). Two Kina clones at `a448fb3` gave gmaps differing in edge order
  plus ~15 bytes (Engram `e63f394`). Verify first: export one commit from two clones under different directory names
  (and with / without a remote) and diff the bytes; suspects are a remote-less RepoId taken from the directory name
  (LB.1) and edge order following walk order. Fix in glia or engram-export, whichever owns the cause, with a gate
  that builds two copies and compares bytes.
- **Re-key pairing through `identity_hint`** (rank 2): an old → new pairing for every re-key the exporter knows,
  endpoints included, with no contract bump ("renamed" questions pass 4 / 9 today).
- **Optional PPR candidate-source side file** (rank 3, lowest priority): data only, a weight-0 feature Engram evaluates;
  never a code dependency, never the read path.
- Keep: `--since` changed-symbol accuracy, leading_doc, the 500-byte cap_prose.
- **Generated-code provenance**: a file whose header says `Code generated … DO NOT EDIT` (and protoc / codegen names:
  `*.pb.go`, `*_grpc.pb.go`, `*_pb2.py`, `*_pb2_grpc.py`, `*.g.dart`, …) gets ORIGIN `generated`, and its
  client call sites mint no GRPC / HTTP client edges (microservices-demo: every Go service "calls" all 9); also the
  python services landing in "(outside projects)" in `glia arch`.
- **Engine RSS creep**: a repeat-`generate()` RSS harness (glia and grpc-go, 12+ rebuilds) comparing glibc,
  glibc + `malloc_trim`, `MALLOC_ARENA_MAX`, mimalloc and jemalloc in glia-py and the CLI; measured during the spec
  run, so the allocator choice (James's call) is made on numbers before D0's dependency commit. Matters for DC.
- **Remove the 0.5.0 aliases**: `patterns_experimental`, `patterns_vs_rev_experimental` and the hidden
  `--experimental` flag (an api_signature / cli_output break).

### DF — language and endpoint deferrals, and the 0.5.1 follow-ups

- PHP short-name symbol imports and group `use \App\{X, Y}`.
- CB.26's residuals: Swift locals / params as receivers, Dart plain-import calls, C++ lambdas / `auto` locals / CMake
  generator expressions, Go route-group prefixes held in a constant.
- WS / gRPC / GraphQL external-endpoint marking (CG.4a / CJ.3 did HTTP).
- `[walk] tests` (CJ.4) feeding the TESTS pass.
- python / eventbus, python / pubsub, ts / pubsub.
- Follow-ups: the Go `package` clause; method-value handlers on typed params / locals; cross-file signal reads binding
  STATE_VARs; OpenAPI `basePath` / `servers` in `spec-status`; Kina's `${this.api}` const-table aliases of config
  expressions.

### DG / DH — the rest of the coverage matrix

Every none (·) and partial (◐) cell at HEAD (`~/.venvs/glia-leap/bin/python bench/substrate-gap/matrix.py`): 69 none +
34 partial = 103 cells. (The spec run's seed lists came from bare `python3`, which imports the user-site 0.4.18 wheel, and
named 152 cells; the verifiers re-graded and found 49 of them already full.) Each is
either fixed (extractor / resolver packet + fixture) or evidenced not-applicable (CF.13a's rule, added to the n/a
list). Unknown (?) cells are out of scope except where a fix's fixture grades one.
- **DG**: python, go, typescript, java, csharp, kotlin, scala, rust.
- **DH**: ruby, php, dart, elixir, swift, c_cpp, clojure, solidity, terraform.

### Cross-cutting (every group reads these)

- Next free ids: node_kind 50, edge_category 37, cell_type 26. D0.1 allocates SAME_AS = 37; any other request goes
  to the integrator with its reason.
- One dependency commit (D0), and only if a dependency is chosen (the allocator). Any Cargo.lock change moves
  PARSER_STAMP. The rules crate is a new workspace member with no external dependency.
- New slots (D0): `rules/` crate; engine `rules`, `watch` (public); activation `algo::classes`; graph resolver
  `identity` (resolvers/mod.rs two lines); py `query`, `watch`, `identity`; cli `query query`, `query identity` (or
  the area the integrator picks), `watch`.
- DF / DG / DH share extractors and per-language parser files; the integrator orders any two packets claiming one file.
- The gated list is unchanged (`SECURITY.md`): no taint / value flow, CVE joins, security zones, sitemap crawling.

## 3. What 0.5.2 does not take

Watch A2 (body-only fast path) and multi-repo watch → 0.5.3. Org-internal package routing → 0.5.3. Stored derived
edges and in-process rule memoisation (R1's R3) → not planned. Engram PPR as a default read path → dropped by Engram.
`decide/`, `glia judge`, precision / calibration → 0.5.4.

## 4. Spec run

One workflow like 0.5.1's: per group a spec agent writes draft packets and an adversarial verifier writes the final
ones (DA, DB, DC, DD, DE, DF, DG, DH); one integrator writes D0.* (ids, slots, the dependency commit if chosen) and
DZ.* (release docs + handoffs), wires cross-group deps and simulates the waves. Outputs merge into
`dev-notes/leap-052-packets.json`, validated by `dev-notes/wave-runner/validate_packets.py`; the runner gains
`--release 052` (schedule_052.py, shared_brief_052.md).

### 4.1 Result (workflows wf_0eab9cd7-9d5 + wf_0703c054-94e, 2026-10-02: 21 agents, 0 errors, ~9.9M subagent tokens)

**153 packets, 41,525 LOC, 15 waves** (W0 = 8 serial D0 packets, then 26 / 26 / 22 / 11 / 10 / 7 / 11 / 7 / 5 / 5 / 5 /
5 / 3 / 2 at the runner's cap of 26). `dev-notes/leap-052-packets.json` holds the packets and the integration report;
`python3 dev-notes/wave-runner/schedule_052.py` reproduces the schedule. Critical path 14 packets after W0, which is the
lower bound (file contention adds no wave): DA.2 → DA.5b → DA.6 → DA.11 → DA.14a → DA.14b → DA.15 (the rules crate) →
DB.2b → DB.2c → DB.3a → DB.3b → DB.8a → DB.8b (check, then the reflexion prelude) → DZ.2.

| group | packets | LOC |
|---|---|---|
| D0 wave 0: SAME_AS = edge 37, the glia-rules crate (the one Cargo.lock change), every slot | 8 | 295 |
| DA the rules crate | 22 | 7,400 |
| DB rules in the engine, `glia query`, GQL, prelude + reflexion parity, rules through the overlay loop (DB.9a–c, Q9) | 19 | 7,130 |
| DC `glia watch` | 13 | 3,705 |
| DD cross-repo identity | 16 | 4,695 |
| DE Engram anchors and hygiene | 11 | 2,500 |
| DF deferrals and follow-ups, + TS typed-receiver inherited calls / concrete-override dispatch (DF.22a–c) | 25 | 6,000 |
| DG / DH the matrix rest | 15 / 22 | 4,000 / 4,870 |
| DZ release docs, three 0.5.2 handoffs | 2 | 930 |

- **Registry:** one id, SAME_AS = edge_category 37; next free node 50 / edge 38 / cell 26. FORMAT_VERSION stays 3.
- **Breaks:** 61 declared (node_id 26, cli_output 24, api_signature 16, edge_removal 15, cell_value 10, qname_shape 7,
  out_of_repo 5), each in its packet's `breaking` block; DZ.2 gates that every one reaches a handoff.
- **No dependency commit.** DE measured the RSS creep (repeat `generate()`, git-archive copies, VmRSS MB first → last):

  | glia, 40 rebuilds | glia, 15 | grpc-go, 15 |
  |---|---|---|
  | glibc 413 → 1,335 (still climbing) | glibc 406 → 852; MALLOC_ARENA_MAX=2 338 → 551; mimalloc 404 → 1,051; jemalloc 384 → 537; jemalloc decay 0 + background_thread 201 → 219 | glibc 211 → 487; mimalloc 215 → 467; jemalloc tuned 111 → 121 |
  | malloc_trim(0) after the swap 208 → 271 | trim 207 → 257; arena 2 + trim 204 → 211 | trim 107 → 158 |

  Trim costs +5..10 % per rebuild; a one-shot CLI build does not creep. DE.10 calls glibc `malloc_trim` when a PyGraph
  drops (no crate, no lock change); DC's WatchSession trims after a swap.
- **Engram determinism, cause found** (DE.1 / DE.2): Engram's second Kina clone had the first clone as its `origin`, so
  LB.1 gave two RepoIds; what remained after sorting was DOCUMENTS edges (an ambiguous bare doc mention picked by a
  RepoId-ordered tie-break) and identity-hint row ties. Both become content tie-breaks.
- **Matrix:** every one of the 103 none / partial cells has an owner packet; none was evidenced not-applicable.
- **Runner:** `gen_wave.py` / `closeout.py --release 052`, `schedule_052.py`, `shared_brief_052.md`,
  `leap-052-corrections.json`.

### 4.2 Decisions for James (the packets took the default shown; override before the wave that implements it)

Worth a look:
1. **Allocator** — none; `malloc_trim` (DE.10). Fallback if overridden: tikv-jemallocator 0.6 in py / cli,
   `cfg(not(target_env = "msvc"))`, decay 0 + background_thread, joining D0.2's lock change.
2. **Concrete overrides as method-level IMPLEMENTS** (DF.22c, W11) — reverses CH.1b's design note "a concrete override
   is not IMPLEMENTS" (a packet author's note, not a ruling). Alternative: method-level INHERITS_FROM plus a new hop in
   liveness, gaps and tests_for.
3. **Q2 applied to the existing kinds** (DB.3a / DB.3d) — a heuristic-only forbid_edge / no_cycle / divergence
   violation stops exiting 1 (cli_output break).
4. **Overlay-loop demotion** (DB.9b) — a graph stanza that adds or hides a declared rule's violation reads review, not
   keep. `overlay propose` drafts no rule (rules come from declared intent, judged by try's check delta).
5. **Generated-code provenance scope** (DE.8) — gRPC client sites only; generated HTTP clients keep their sites.
6. **Engram to confirm** — DE.4 refuses word-less renames (precision over recall, CK.2's rule); DE.5's PPR side-file
   format (droppable).
7. **qname breaks** — DG.10 Akka routes compose their path; DG.12 sbt-member kinds gain ` @<dir>`.

Defaults taken without a question: DEFAULT_MAX_ROWS 5,000,000 (stops both measured glia×10 closures); DA.17 gates the
evaluator's own RSS peak; `glia query` exits 1 on an empty answer; `glia watch` in the change area, persist every 60 s,
poll 500 ms, debounce 200 ms, SIGINT through a declared `signal(2)` extern (no crate); graft memos in memory only; an
overlay edit rebuilds every group; package-name classes collapse without walking, SAME_AS weighs 0 in PPR; a contract op
anchors to its HTTP-paired route (DERIVED); `--no-collapse` returns the raw rows; the persisted star centre is the least
(qname, kind, graph index) member; an ambiguous doc mention keeps one weak edge to the least qname; DF.14's Pub/Sub
binding is a JSON CODE cell (no id); DF.17 TS only; DF.19 `annotated_base`; DF.20 folds a config `.replace` (constant
folding, not data-flow); DF.2's short-name refusal PHP only; DF.15 mints no PACKAGE node; DG.11 names a Scala test by its
description; DG.2 skips only ProducerRecord envelopes; DH.13 / DH.17b / DH.18 follow each library's own key / model rule.

### 4.3 Found during the spec run, no owner (0.5.3 candidates unless James pulls one in)

- Go generic instantiation calls `F[T](...)` mint no CALLS.
- Angular lifecycle hooks and constructors of reached components are dead_symbol rows (wpf: 312 ngOnInit, 942
  constructors in 3,877 rows); TS interface members are not METHOD nodes, so interface-typed receivers bind nothing.
- TS: call-initialised locals have no type (`return_types` is Go-only); module-level instance consts read in a
  function; statics inherited through a subclass name; superclass STATE_VARs through a typed receiver; default-imported
  class types.
- A `[[constraint]]` with no anchor and no PROJECT scope is orphaned silently; `[walk] tests` is invisible to a layout
  merge, like every overlay section.
- The engine's tier vocabularies stay three (+ glia_rules::value::Tier); a witness path among equal alternatives can
  differ between checkouts; RepoId-ordered tie-breaks remain in pick_primary, cochange, overlay bind, declared, cells.
- Per-group remainders (integration_report.gaps_no_owner): DD maven / gradle / pubspec / mix / csproj PACKAGE_DEPs; DF
  tRPC external marking, Swift protocol receivers, Dart export barrels; DG Rust trait members, Juniper / Sangria; DH
  Elixir one-line defs, Absinthe import_fields, Ruby mixins.
